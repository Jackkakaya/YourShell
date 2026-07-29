#ifndef ASHELL_BRIDGING_H
#define ASHELL_BRIDGING_H

#include <stddef.h>
#include <stdint.h>

typedef void (*ashell_output_cb)(void *ctx, const uint8_t *bytes, size_t len);
typedef void (*ashell_done_cb)(void *ctx, int32_t exit_code, const char *cwd);

void *ashell_session_new(ashell_output_cb out_cb, ashell_done_cb done_cb,
                         void *ctx, const char *working_dir);
void ashell_exec(void *session, const char *cmd);
char *ashell_complete(void *session, const char *line, size_t cursor);
void ashell_stdin_write(void *session, const uint8_t *bytes, size_t len);
void ashell_stdin_eof(void *session);
void ashell_session_free(void *session);
char *ashell_selftest(const char *working_dir);
void ashell_string_free(char *s);
uint32_t ashell_abi_version(void);

/* Agent / non-interactive path: run a command to completion capturing
 * stdout/stderr separately. timeout_ms == 0 means no timeout. Free with
 * ashell_capture_free. ashell_cancel interrupts the running command. */
typedef struct {
    int32_t exit_code;
    char *stdout_str;
    char *stderr_str;
} ashell_capture_result;

ashell_capture_result *ashell_run_capture(void *session, const char *cmd,
                                          uint64_t timeout_ms);
void ashell_cancel(void *session);
void ashell_capture_free(ashell_capture_result *result);

void ys_node_start_resident(const char *main_js_path);

typedef int32_t (*ashell_ios_copy_cb)(const uint8_t *bytes, size_t len);
typedef void (*ashell_ios_paste_output_cb)(void *ctx, const uint8_t *bytes, size_t len);
typedef int32_t (*ashell_ios_paste_cb)(void *ctx, ashell_ios_paste_output_cb output);
typedef int32_t (*ashell_ios_open_cb)(const uint8_t *bytes, size_t len);
int32_t ashell_ios_host_install(ashell_ios_copy_cb copy, ashell_ios_paste_cb paste,
                                ashell_ios_open_cb open);

#endif
