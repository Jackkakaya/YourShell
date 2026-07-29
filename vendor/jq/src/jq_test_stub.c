/*
 * The upstream CLI references jq_testsuite() for the maintainer-only
 * --run-tests option. jq_test.c is intentionally not shipped in this embedded
 * runtime, so provide a contained response instead of leaving every final
 * executable with an unresolved symbol.
 */
#include <stdio.h>
#include "jv.h"

int jq_testsuite(jv lib_dirs, int verbose, int argc, char *argv[]) {
  (void)lib_dirs;
  (void)verbose;
  (void)argc;
  (void)argv;
  fputs("jq: --run-tests is unavailable in this embedded build\n", stderr);
  return 2;
}
