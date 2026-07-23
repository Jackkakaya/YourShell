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
static PyThreadState *ys_main_tstate = NULL;
static pthread_mutex_t ys_py_mutex = PTHREAD_MUTEX_INITIALIZER;

// Command driver: parses the python CLI subset we support and dispatches,
// capturing the exit code into sys._ys_exit_code.
static const char *YS_DRIVER =
    "import sys\n"
    "import os as _ys_os\n"
    "_ys_site = _ys_os.environ.get('YOURSHELL_PY_SITE')\n"
    "if _ys_site:\n"
    "    _ys_os.makedirs(_ys_site, exist_ok=True)\n"
    "    if _ys_site not in sys.path:\n"
    "        sys.path.insert(0, _ys_site)\n"
    "def _ys_main():\n"
    "    args = sys.argv[1:]\n"
    "    if not args:\n"
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
    "        runpy.run_module(args[1], run_name='__main__', alter_sys=True)\n"
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

    PyPreConfig_InitIsolatedConfig(&preconfig);
    preconfig.utf8_mode = 1;
    status = Py_PreInitialize(&preconfig);
    if (PyStatus_Exception(status)) {
        return -1;
    }

    PyConfig_InitIsolatedConfig(&config);
    config.buffered_stdio = 0;          // session pipes want bytes immediately
    config.use_system_logger = 0;       // iOS default routes stdio to os_log;
                                        // we want the real fds (session pipes)
    config.write_bytecode = 0;          // bundle is signed/read-only
    config.install_signal_handlers = 0; // the shell owns signal handling
    config.user_site_directory = 1;     // allow pip --user installs

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

    ys_main_tstate = PyEval_SaveThread();
    ys_py_initialized = 1;
    return 0;
}

int ys_python_run(int argc, const char **argv) {
    pthread_mutex_lock(&ys_py_mutex);
    if (ys_python_ensure_init() != 0) {
        pthread_mutex_unlock(&ys_py_mutex);
        return 125;
    }

    // Attach the main interpreter to this thread, then detach so the
    // subinterpreter can own the thread.
    PyEval_RestoreThread(ys_main_tstate);
    PyThreadState *mainstate = PyThreadState_Get();
    PyThreadState_Swap(NULL);

    PyInterpreterConfig icfg = {
        .use_main_obmalloc = 0,
        .allow_fork = 0,
        .allow_exec = 0,
        .allow_threads = 1,
        .allow_daemon_threads = 0,
        .check_multi_interp_extensions = 1,
        .gil = PyInterpreterConfig_OWN_GIL,
    };
    PyThreadState *sub = NULL;
    PyStatus st = Py_NewInterpreterFromConfig(&sub, &icfg);
    int exit_code = 125;

    if (!PyStatus_Exception(st) && sub != NULL) {
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

        Py_EndInterpreter(sub);
    }

    PyThreadState_Swap(mainstate);
    ys_main_tstate = PyEval_SaveThread();

    pthread_mutex_unlock(&ys_py_mutex);
    return exit_code;
}
