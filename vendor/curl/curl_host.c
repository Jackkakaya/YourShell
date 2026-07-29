#include <setjmp.h>
#include <stdio.h>
#include <stdlib.h>
#include <curl/curl.h>

extern int ys_curl_main(int argc, char **argv);

/*
 * curl is entered under command_host's process-state lock, so one jump target
 * is sufficient. Normal curl invocations return through upstream cleanup;
 * fatal/help/version paths which call exit() are contained here.
 */
static jmp_buf ys_curl_exit_target;
static int ys_curl_exit_code;

_Noreturn void ys_curl_exit(int code) {
  fflush(NULL);
  ys_curl_exit_code = code;
  longjmp(ys_curl_exit_target, 1);
}

int ys_curl_run(int argc, char **argv) {
  ys_curl_exit_code = 0;
  if (setjmp(ys_curl_exit_target) != 0) {
    curl_global_cleanup();
    return ys_curl_exit_code;
  }
  return ys_curl_main(argc, argv);
}
