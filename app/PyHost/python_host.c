//
// python_host.c — owns the embedded CPython runtime for YourShell.
//
// The runtime is initialized once, lazily, with PYTHONHOME taken from the
// YOURSHELL_PYTHON_HOME environment variable (set by the Swift side to the
// bundled stdlib). Every `python3` command then executes in a fresh
// subinterpreter with its own GIL (PEP 684), so command invocations are
// isolated from each other. The caller (Rust core) has already pointed
// process fd 0/1/2 at the session pipes and holds the process-state lock.
//
// The subinterpreter dance follows CPython's own canonical embedding
// pattern (Modules/_testcapimodule.c, run_in_subinterp_with_config).
//

#include <Python/Python.h>
#include <pthread.h>
#include <stdlib.h>

static int ys_py_initialized = 0;
static pthread_mutex_t ys_py_mutex = PTHREAD_MUTEX_INITIALIZER;

// Command driver: parses the python CLI subset we support and dispatches,
// capturing the exit code into sys._ys_exit_code.
static const char *YS_DRIVER =
    "import sys\n"
    "import os as _ys_os\n"
    "import io as _ys_io\n"
    // The persistent interpreter caches sys.std* file objects at init, but the
    // Rust adapter dup2s fresh session fds onto 0/1/2 for each command. Rebind
    // the streams to the current fds every command (closefd=False so we never
    // close the session's fd). Without this, once the first command's fd
    // closes, every later command sees 'I/O operation on closed file'.
    "try:\n"
    "    sys.stdin = _ys_io.TextIOWrapper(_ys_io.FileIO(0, 'r', closefd=False), encoding='utf-8')\n"
    "    sys.stdout = _ys_io.TextIOWrapper(_ys_io.FileIO(1, 'w', closefd=False), encoding='utf-8', write_through=True)\n"
    "    sys.stderr = _ys_io.TextIOWrapper(_ys_io.FileIO(2, 'w', closefd=False), encoding='utf-8', write_through=True)\n"
    "except Exception:\n"
    "    pass\n"
    // Apply the per-command exported env the Rust adapter wrote (os.environ is
    // frozen at init for the persistent interpreter).
    "_ys_ef = _ys_os.environ.get('YS_PY_ENV_FILE')\n"
    "if _ys_ef:\n"
    "    try:\n"
    "        import json as _ys_json\n"
    "        with open(_ys_ef) as _f:\n"
    "            for _k, _v in _ys_json.load(_f).items():\n"
    "                _ys_os.environ[_k] = _v\n"
    "    except Exception:\n"
    "        pass\n"
    "_ys_site = _ys_os.environ.get('YOURSHELL_PY_SITE')\n"
    "if _ys_site:\n"
    "    _ys_os.makedirs(_ys_site, exist_ok=True)\n"
    "    if _ys_site not in sys.path:\n"
    "        sys.path.insert(0, _ys_site)\n"
    "_ys_legacy = _ys_os.environ.get('YOURSHELL_PY_LEGACY_SITE')\n"
    "if _ys_legacy and _ys_os.path.isdir(_ys_legacy) and _ys_legacy not in sys.path:\n"
    "    sys.path.append(_ys_legacy)\n"
    // Read-only prebundled site shipped inside the app (cross-compiled iOS
    // wheels: lxml/Pillow + pure-Python office libs). No makedirs (bundle is
    // read-only); appended after the writable site so a user pip-installed copy
    // wins over the bundled one.
    "_ys_pre = _ys_os.environ.get('YOURSHELL_PY_PREBUNDLED')\n"
    "if _ys_pre:\n"
    "    for _ys_pre_dir in _ys_pre.split(_ys_os.pathsep):\n"
    "        if _ys_os.path.isdir(_ys_pre_dir) and _ys_pre_dir not in sys.path:\n"
    "            sys.path.append(_ys_pre_dir)\n"
    // sys.executable: iOS has no python binary. pip builds spawn commands as
    // [sys.executable, ...]; point it at a stub (created so os.path.exists passes)
    // so the in-process subprocess shim recognizes 'run our python' and diverts
    // it in-process instead of fork/exec (which iOS forbids).
    "try:\n"
    "    _ys_exe = _ys_os.path.join(_ys_site or _ys_os.environ.get('HOME', '/tmp'), 'ys-python3')\n"
    "    if not _ys_os.path.exists(_ys_exe):\n"
    "        open(_ys_exe, 'w').close()\n"
    "    sys.executable = _ys_exe\n"
    "    sys._base_executable = _ys_exe\n"
    "except Exception:\n"
    "    pass\n"
    // Install the in-process subprocess shim: pip / PEP 517 build backends spawn
    // python subprocesses for build isolation and hooks; iOS bans fork, so route
    // self-python spawns in-process. Lets pure-Python sdists `pip install`.
    "try:\n"
    "    import _ys_subprocess as _ys_sp\n"
    "    _ys_sp.install()\n"
    "except Exception:\n"
    "    pass\n"
    "def _ys_main():\n"
    "    args = sys.argv[1:]\n"
    "    if not args:\n"
    "        if _ys_os.environ.get('YS_STDIN_TTY') == '1':\n"
    "            import code\n"
    "            code.interact(banner='Python ' + sys.version.split()[0]\n"
    "                          + ' (YourShell, in-process)', exitmsg='')\n"
    "            return 0\n"
    "        src = sys.stdin.read()\n"
    "        sys.argv = ['-']\n"
    "        exec(compile(src, '<stdin>', 'exec'), {'__name__': '__main__'})\n"
    "        return 0\n"
    "    a = args[0]\n"
    "    if a in ('--version', '-V'):\n"
    "        print('Python ' + sys.version.split()[0])\n"
    "        return 0\n"
    "    if a == '-c':\n"
    "        if len(args) < 2:\n"
    "            print('python3: -c requires an argument', file=sys.stderr)\n"
    "            return 2\n"
    "        sys.argv = ['-c'] + args[2:]\n"
    "        exec(compile(args[1], '<string>', 'exec'), {'__name__': '__main__'})\n"
    "        return 0\n"
    "    if a == '-m':\n"
    "        if len(args) < 2:\n"
    "            print('python3: -m requires an argument', file=sys.stderr)\n"
    "            return 2\n"
    "        import runpy\n"
    "        sys.argv = [args[1]] + args[2:]\n"
    // pip 26 installs a sys audit hook during `install` which cannot be
    // removed afterwards. In embedded CPython that hook survives into later
    // commands and emits a warning for every third-party import. A standalone
    // pip process would exit immediately, so suppress hook registration only
    // for the process-emulated pip invocation.
    "        if args[1] == 'pip':\n"
    "            _ys_addaudit = sys.addaudithook\n"
    "            sys.addaudithook = lambda _hook: None\n"
    "            try:\n"
    "                runpy.run_module(args[1], run_name='__main__', alter_sys=True)\n"
    "            finally:\n"
    "                sys.addaudithook = _ys_addaudit\n"
    "        else:\n"
    "            runpy.run_module(args[1], run_name='__main__', alter_sys=True)\n"
    "        return 0\n"
    "    import runpy\n"
    "    sys.argv = args\n"
    "    runpy.run_path(a, run_name='__main__')\n"
    "    return 0\n"
    "try:\n"
    "    _ys_rc = _ys_main() or 0\n"
    "except SystemExit as _e:\n"
    "    _ys_rc = _e.code if isinstance(_e.code, int) else (0 if _e.code is None else 1)\n"
    "except BaseException:\n"
    "    import traceback\n"
    "    traceback.print_exc()\n"
    "    _ys_rc = 1\n"
    "sys._ys_exit_code = _ys_rc\n";

static int ys_python_ensure_init(void) {
    if (ys_py_initialized) {
        return 0;
    }

    PyStatus status;
    PyPreConfig preconfig;
    PyConfig config;

    // User-site must be enabled at pre-initialization time too. Combining an
    // isolated preconfig with a non-isolated PyConfig leaves
    // sys.flags.no_user_site set, making pip --user reject a writable sandbox.
    PyPreConfig_InitPythonConfig(&preconfig);
    preconfig.utf8_mode = 1;
    status = Py_PreInitialize(&preconfig);
    if (PyStatus_Exception(status)) {
        return -1;
    }

    PyConfig_InitIsolatedConfig(&config);
    // Isolated config forces -I (= -s -E), which hard-disables user-site
    // (sys.flags.no_user_site=1) regardless of user_site_directory. Turn isolated
    // off and re-enable env so `pip install --user` works: --user resolves against
    // the whole environment (so prebundled numpy/pandas/… are treated as installed
    // and NOT rebuilt from source), installing new packages under PYTHONUSERBASE.
    // Also lets the user's own PYTHONPATH take effect (a-Shell-like).
    config.isolated = 0;
    config.use_environment = 1;
    config.buffered_stdio = 0;          // session pipes want bytes immediately
    config.use_system_logger = 0;       // iOS default routes stdio to os_log;
                                        // we want the real fds (session pipes)
    config.write_bytecode = 0;          // bundle is signed/read-only
    config.install_signal_handlers = 0; // the shell owns signal handling
    config.user_site_directory = 1;     // enable pip --user installs

    const char *home = getenv("YOURSHELL_PYTHON_HOME");
    if (home != NULL) {
        wchar_t *whome = Py_DecodeLocale(home, NULL);
        if (whome != NULL) {
            PyConfig_SetString(&config, &config.home, whome);
            PyMem_RawFree(whome);
        }
    }

    status = PyConfig_Read(&config);
    if (PyStatus_Exception(status)) {
        PyConfig_Clear(&config);
        return -1;
    }
    status = Py_InitializeFromConfig(&config);
    PyConfig_Clear(&config);
    if (PyStatus_Exception(status)) {
        return -1;
    }

    // Release the GIL and let PyGILState manage per-thread states from here.
    PyEval_SaveThread();
    ys_py_initialized = 1;
    return 0;
}

int ys_python_run(int argc, const char **argv) {
    pthread_mutex_lock(&ys_py_mutex);
    if (ys_python_ensure_init() != 0) {
        pthread_mutex_unlock(&ys_py_mutex);
        return 125;
    }

    // Commands arrive on arbitrary threads (tokio's blocking pool), so acquire
    // the GIL with PyGILState_Ensure, which creates/uses a thread state bound
    // to THIS OS thread. Reusing one saved PyThreadState across different
    // threads is undefined and corrupts the GC (bus error in dealloc).
    //
    // Execute in the persistent main interpreter: per-command isolation comes
    // from the driver running user code in a fresh __main__-style globals dict,
    // while module state (sys.modules) deliberately persists — major C
    // extensions (lxml, numpy) refuse to load into more than one interpreter
    // per process. Same model as a-Shell / Jupyter kernels.
    PyGILState_STATE gil = PyGILState_Ensure();

    int exit_code = 125;
    PyObject *py_argv = PyList_New(argc);
    if (py_argv != NULL) {
        for (int i = 0; i < argc; i++) {
            PyList_SetItem(py_argv, i, PyUnicode_DecodeFSDefault(argv[i]));
        }
        PySys_SetObject("argv", py_argv);
        Py_DECREF(py_argv);
    }

    int rc = PyRun_SimpleString(YS_DRIVER);
    exit_code = (rc == 0) ? 0 : 1;
    PyObject *code_obj = PySys_GetObject("_ys_exit_code"); // borrowed
    if (code_obj != NULL && PyLong_Check(code_obj)) {
        exit_code = (int)PyLong_AsLong(code_obj);
    }

    PyGILState_Release(gil);

    pthread_mutex_unlock(&ys_py_mutex);
    return exit_code;
}
