#include <setjmp.h>

extern int ys_bsdunzip_main(int argc, char **argv);
static jmp_buf *ys_unzip_jump;
static int ys_unzip_status;

_Noreturn void ys_bsdunzip_exit(int status)
{
    ys_unzip_status = status;
    if (ys_unzip_jump != 0)
        longjmp(*ys_unzip_jump, 1);
    __builtin_trap();
}

int ys_bsdunzip_run(int argc, char **argv)
{
    jmp_buf jump;
    jmp_buf *previous = ys_unzip_jump;
    int status;
    ys_unzip_jump = &jump;
    ys_unzip_status = 0;
    if (setjmp(jump) == 0)
        status = ys_bsdunzip_main(argc, argv);
    else
        status = ys_unzip_status;
    ys_unzip_jump = previous;
    return status;
}
