/*
 * Host boundary for libarchive's official bsdtar CLI.
 *
 * Upstream is process-shaped and may call exit(). YourShell is a long-lived
 * process, so convert that exit into a return from this invocation. The CLI
 * parser and command behavior remain entirely upstream.
 */
#include <setjmp.h>
#include <signal.h>

extern int ys_bsdtar_main(int argc, char **argv);

static _Thread_local jmp_buf *ys_bsdtar_jump;
static _Thread_local int ys_bsdtar_status;

_Noreturn void
ys_bsdtar_exit(int status)
{
    ys_bsdtar_status = status;
    if (ys_bsdtar_jump != 0)
        longjmp(*ys_bsdtar_jump, 1);
    __builtin_trap();
}

int
ys_bsdtar_run(int argc, char **argv)
{
    jmp_buf jump;
    jmp_buf *previous = ys_bsdtar_jump;
    struct sigaction old_pipe, old_usr1;
    int have_pipe = sigaction(SIGPIPE, 0, &old_pipe) == 0;
    int have_usr1 = sigaction(SIGUSR1, 0, &old_usr1) == 0;
    int status;

    ys_bsdtar_jump = &jump;
    ys_bsdtar_status = 0;
    if (setjmp(jump) == 0)
        status = ys_bsdtar_main(argc, argv);
    else
        status = ys_bsdtar_status;
    ys_bsdtar_jump = previous;

    /* bsdtar installs process-wide handlers as a normal CLI would. */
    if (have_pipe)
        sigaction(SIGPIPE, &old_pipe, 0);
    if (have_usr1)
        sigaction(SIGUSR1, &old_usr1, 0);
    return status;
}
