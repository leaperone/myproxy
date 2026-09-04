#pragma once

#ifdef __cplusplus
extern "C" {
#endif

void myproxy_ne_free_string(char *value);

/* 0 running, 2 reboot required, -1 error (error_out set). */
int myproxy_ne_enable(const char *json, char **error_out);
int myproxy_ne_disable(char **error_out);

#ifdef __cplusplus
}
#endif
