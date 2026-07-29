#include <setjmp.h>

extern int ys_bsd_gzip_main(int argc, char **argv);

/* command_host serializes process-shaped commands, so plain static state is
 * sufficient and also works with older iOS targets without C TLS support. */
static jmp_buf *ys_gzip_jump;
static int ys_gzip_status;

_Noreturn void ys_bsd_gzip_exit(int status)
{
    ys_gzip_status = status;
    if (ys_gzip_jump != 0)
        longjmp(*ys_gzip_jump, 1);
    __builtin_trap();
}

int ys_bsd_gzip_run(int argc, char **argv)
{
    jmp_buf jump;
    jmp_buf *previous = ys_gzip_jump;
    int status;

    ys_gzip_jump = &jump;
    ys_gzip_status = 0;
    if (setjmp(jump) == 0)
        status = ys_bsd_gzip_main(argc, argv);
    else
        status = ys_gzip_status;
    ys_gzip_jump = previous;
    return status;
}
