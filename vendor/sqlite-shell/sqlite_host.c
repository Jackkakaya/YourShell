#include <setjmp.h>
#include <stdio.h>
#include <stdlib.h>
#include <errno.h>

/*
 * shell.c is a process-shaped program. A few fatal/error paths call exit(),
 * and main() registers terminal cleanup with atexit(). Neither is valid in a
 * long-lived iOS host process, so the build renames those calls here.
 *
 * SQLite commands are serialized by command_host's process-state lock. A
 * process-global jump target is therefore sufficient and avoids pretending
 * this wrapper is independently thread-safe. The embedded shell must never
 * call sqlite3_shutdown(): YourAI's backend uses the same process-wide SQLite
 * library concurrently, and shutting it down invalidates the host's caches.
 */
static jmp_buf ys_sqlite_exit_target;
static int ys_sqlite_exit_code;

extern int ys_sqlite3_main(int argc, char **argv);
extern void ys_sqlite3_reset_process_state(void);

_Noreturn void ys_sqlite3_exit(int code) {
  fflush(NULL);
  ys_sqlite_exit_code = code;
  longjmp(ys_sqlite_exit_target, 1);
}

int ys_sqlite3_atexit(void (*callback)(void)) {
  /*
   * Registering once per invocation would retain callbacks until the app
   * exits. On Darwin/iOS the shell does not change Win32 console state, and
   * command_host restores fds itself, so no process-exit callback is needed.
   */
  (void)callback;
  return 0;
}

int ys_sqlite3_system(const char *command) {
  (void)command;
  errno = ENOSYS;
  return -1;
}

int ys_sqlite3_run(int argc, char **argv) {
  ys_sqlite_exit_code = 0;
  if (setjmp(ys_sqlite_exit_target) != 0) {
    ys_sqlite3_reset_process_state();
    return ys_sqlite_exit_code;
  }
  int code = ys_sqlite3_main(argc, argv);
  fflush(NULL);
  ys_sqlite3_reset_process_state();
  return code;
}
