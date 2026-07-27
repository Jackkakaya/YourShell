"""In-process ``subprocess`` shim for YourShell on iOS (no fork/exec).

iOS forbids ``fork``/``posix_spawn`` (``[Errno 45] ios does not support
processes``). ``pip`` and PEP 517 build backends spawn ``[sys.executable, ...]``
subprocesses for build isolation and to run the build backend hooks. Those all
re-invoke *our own* Python interpreter, so we can run them **in-process**
(synchronously, on the same persistent interpreter, with saved/restored ``sys``
state and captured I/O) instead of spawning a real process.

Only invocations of our own Python are emulated. Any other executable (a C
compiler, ``git``, …) still fails — the same limitation a-Shell has: pure-Python
packages install, C-extension source builds do not (use the prebundled wheels
or ``git clone`` instead).

Installed by ``install()`` (called once from the Python driver at startup). It
monkeypatches ``subprocess.Popen``; ``run``/``call``/``check_output`` are all
built on it, so they inherit the behaviour.
"""

import io
import os
import runpy
import shlex
import sys

import subprocess as _sp

_RealPopen = _sp.Popen
_installed = False


def _is_our_python(exe):
    """True if *exe* refers to this interpreter (so we can run it in-process)."""
    if not exe:
        return False
    exe = str(exe)
    if sys.executable and os.path.realpath(exe) == os.path.realpath(sys.executable):
        return True
    base = os.path.basename(exe)
    return base in ("python", "python3", "python3.14") or base.startswith("python3")


def _dispatch(args):
    """Parse a Python argv (minus argv[0]) into (mode, payload, rest).

    Mirrors the driver: skips leading interpreter flags (``-I``/``-E``/``-s``/
    ``-u``…, including combined bundles like ``-Im``), then finds ``-c CODE`` /
    ``-m MODULE`` / a script path / ``-`` (stdin).
    """
    i, n = 0, len(args)
    # Flags that consume the following argument.
    takes_arg = {"W", "X"}
    while i < n:
        a = args[i]
        if a == "-c":
            return "c", args[i + 1], args[i + 2:]
        if a == "-m":
            return "m", args[i + 1], args[i + 2:]
        if a == "-":
            return "stdin", None, args[i + 1:]
        if a.startswith("-") and len(a) > 1:
            j = 1
            while j < len(a):
                ch = a[j]
                if ch == "c":
                    if j + 1 < len(a):
                        return "c", a[j + 1:], args[i + 1:]
                    return "c", args[i + 1], args[i + 2:]
                if ch == "m":
                    if j + 1 < len(a):
                        return "m", a[j + 1:], args[i + 1:]
                    return "m", args[i + 1], args[i + 2:]
                if ch in takes_arg:
                    if j + 1 < len(a):
                        j = len(a)  # value is the tail of this token
                    else:
                        i += 1      # value is the next argv item
                    break
                j += 1
            i += 1
            continue
        # First non-flag token → a script path.
        return "path", a, args[i + 1:]
    return "none", None, []


def _run_inproc(mode, payload, rest, stdin_bytes, cwd, env):
    """Run a Python invocation in this interpreter; return (rc, out, err) bytes."""
    old = (sys.argv, list(sys.path), os.getcwd(),
           sys.stdout, sys.stderr, sys.stdin, dict(os.environ))
    out_buf, err_buf = io.BytesIO(), io.BytesIO()
    out = io.TextIOWrapper(out_buf, encoding="utf-8", write_through=True)
    err = io.TextIOWrapper(err_buf, encoding="utf-8", write_through=True)
    rc = 0
    try:
        if cwd:
            os.chdir(cwd)
        if env is not None:
            os.environ.clear()
            os.environ.update(env)
        sys.stdout, sys.stderr = out, err
        if stdin_bytes is not None:
            sys.stdin = io.TextIOWrapper(io.BytesIO(stdin_bytes), encoding="utf-8")
        try:
            if mode == "c":
                sys.argv = ["-c"] + list(rest)
                exec(compile(payload, "<string>", "exec"), {"__name__": "__main__"})
            elif mode == "m":
                sys.argv = [payload] + list(rest)
                runpy.run_module(payload, run_name="__main__", alter_sys=True)
            elif mode == "path":
                sys.argv = [payload] + list(rest)
                runpy.run_path(payload, run_name="__main__")
            elif mode == "stdin":
                src = (stdin_bytes or b"").decode("utf-8")
                sys.argv = ["-"] + list(rest)
                exec(compile(src, "<stdin>", "exec"), {"__name__": "__main__"})
        except SystemExit as e:
            rc = e.code if isinstance(e.code, int) else (0 if e.code is None else 1)
        except BaseException:  # noqa: BLE001 — mirror a real process crashing
            import traceback
            traceback.print_exc()
            rc = 1
    finally:
        try:
            out.flush()
            err.flush()
        except Exception:
            pass
        (sys.argv, path, cwd0, sys.stdout, sys.stderr, sys.stdin, environ0) = old
        sys.path[:] = path
        try:
            os.chdir(cwd0)
        except Exception:
            pass
        os.environ.clear()
        os.environ.update(environ0)
    return rc, out_buf.getvalue(), err_buf.getvalue()


class _StdinBuf(io.BytesIO):
    """Writable stdin buffer. pip asserts ``proc.stdin`` then closes it; the PEP
    517 hook passes its args via a control dir, not stdin, so we just retain
    whatever (if anything) was written and hand it to the in-process run."""

    saved = b""

    def close(self):
        try:
            self.saved = self.getvalue()
        except Exception:
            pass
        super().close()

    def value(self):
        try:
            return self.getvalue()
        except Exception:
            return self.saved


class _LazyReader:
    """proc.stdout / proc.stderr proxy that triggers the (deferred) in-process
    run on first read, so callers can write+close proc.stdin first."""

    def __init__(self, popen, which):
        self._p = popen
        self._which = which

    def _s(self):
        self._p._ensure_ran()
        return self._p._out_stream if self._which == "out" else self._p._err_stream

    def read(self, *a):
        return self._s().read(*a)

    def readline(self, *a):
        return self._s().readline(*a)

    def readlines(self, *a):
        return self._s().readlines(*a)

    def __iter__(self):
        return iter(self._s())

    def close(self):
        try:
            self._s().close()
        except Exception:
            pass


class _ShimPopen:
    """Popen-compatible object that runs our Python in-process (else defers to
    the real Popen, which raises the iOS 'no processes' error)."""

    def __init__(self, args, bufsize=-1, executable=None, stdin=None, stdout=None,
                 stderr=None, preexec_fn=None, close_fds=True, shell=False,
                 cwd=None, env=None, universal_newlines=None, startupinfo=None,
                 creationflags=0, restore_signals=True, start_new_session=False,
                 pass_fds=(), *, text=None, encoding=None, errors=None, **kwargs):
        argv = shlex.split(args) if isinstance(args, str) else list(args)
        exe = executable or (argv[0] if argv else None)
        if shell or not _is_our_python(exe):
            self._real = _RealPopen(
                args, bufsize=bufsize, executable=executable, stdin=stdin,
                stdout=stdout, stderr=stderr, cwd=cwd, env=env,
                universal_newlines=universal_newlines, text=text,
                encoding=encoding, errors=errors, **kwargs)
            return

        self._real = None
        self.args = args
        self.pid = -1
        self.returncode = None
        self._argv = argv
        self._cwd = cwd
        self._env = env
        # subprocess is in text mode if ANY of text/encoding/errors/
        # universal_newlines is set — `errors` alone (pip uses it) counts.
        self._text = bool(text or universal_newlines or encoding or errors)
        self._encoding = encoding or "utf-8"
        self._want_out = stdout == _sp.PIPE
        self._want_err = stderr == _sp.PIPE
        self._merge_err = stderr == _sp.STDOUT
        self._ran = False
        self._out_stream = None
        self._err_stream = None
        # Deferred execution so pip can write+close proc.stdin before we run.
        self.stdin = _StdinBuf() if stdin == _sp.PIPE else None
        self.stdout = _LazyReader(self, "out") if self._want_out else None
        self.stderr = _LazyReader(self, "err") if (self._want_err and not self._merge_err) else None

    # -- passthrough proxy when we delegated to the real Popen --
    def __getattr__(self, name):
        real = self.__dict__.get("_real")
        if real is not None:
            return getattr(real, name)
        raise AttributeError(name)

    def _ensure_ran(self):
        if self._ran:
            return
        self._ran = True
        stdin_bytes = self.stdin.value() if isinstance(self.stdin, _StdinBuf) else None
        if not stdin_bytes:
            stdin_bytes = None
        mode, payload, rest = _dispatch(self._argv[1:])
        if mode == "none":
            rc, ob, eb = 0, b"", b""
        else:
            rc, ob, eb = _run_inproc(mode, payload, rest, stdin_bytes, self._cwd, self._env)
        self.returncode = rc
        if self._merge_err:
            ob, eb = ob + eb, b""
        enc = self._encoding
        if not self._want_out and ob:
            try:
                sys.stdout.write(ob.decode(enc, "replace"))
                sys.stdout.flush()
            except Exception:
                pass
        if not self._want_err and eb and not self._merge_err:
            try:
                sys.stderr.write(eb.decode(enc, "replace"))
                sys.stderr.flush()
            except Exception:
                pass
        self._out_cache = ob
        self._err_cache = eb
        self._out_stream = io.StringIO(ob.decode(enc, "replace")) if self._text else io.BytesIO(ob)
        self._err_stream = io.StringIO(eb.decode(enc, "replace")) if self._text else io.BytesIO(eb)

    def communicate(self, input=None, timeout=None):
        if self._real is not None:
            return self._real.communicate(input, timeout)
        if input is not None and isinstance(self.stdin, _StdinBuf) and not self._ran:
            data = input if isinstance(input, (bytes, bytearray)) else input.encode(self._encoding)
            self.stdin.saved = bytes(data)
        self._ensure_ran()
        enc = self._encoding
        out = (self._out_cache.decode(enc, "replace") if self._text else self._out_cache) if self._want_out else None
        err = (self._err_cache.decode(enc, "replace") if self._text else self._err_cache) if (self._want_err and not self._merge_err) else None
        return (out, err)

    def wait(self, timeout=None):
        if self._real is not None:
            return self._real.wait(timeout)
        self._ensure_ran()
        return self.returncode

    def poll(self):
        if self._real is not None:
            return self._real.poll()
        return self.returncode if self._ran else None

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.wait()

    def kill(self):
        pass

    terminate = kill

    def send_signal(self, sig):
        pass


def install():
    """Idempotently monkeypatch subprocess so our-Python spawns run in-process."""
    global _installed
    if _installed:
        return
    _sp.Popen = _ShimPopen
    _installed = True
