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
void ashell_session_free(void *session);
char *ashell_selftest(const char *working_dir);
void ashell_string_free(char *s);

void ys_node_start_resident(const char *main_js_path);

#endif
