#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void myproxy_ne_free_string(char *value);

/* Pure validation: 0 valid, -1 rejected. Does not start or stop providers. */
int myproxy_ne_validate(const char *json, char **error_out);

/* 1 submitted, -1 rejected (error_out set). Read status for actual readiness. */
int myproxy_ne_enable(const char *json, char **error_out);
int myproxy_ne_disable(uint64_t operation_revision, char **error_out);
/* JSON snapshot; caller frees with myproxy_ne_free_string. */
char *myproxy_ne_status(void);
/* Slim activity JSON for connection-process join; caller frees with myproxy_ne_free_string. */
char *myproxy_ne_activity_batch(uint64_t cursor, uint32_t limit);
/* Service native callbacks during bounded CLI waits; not used by the GUI. */
void myproxy_ne_wait(uint32_t milliseconds);

#ifdef __cplusplus
}
#endif
