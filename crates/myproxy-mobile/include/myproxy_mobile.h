#ifndef MYPROXY_MOBILE_H
#define MYPROXY_MOBILE_H
#ifdef __cplusplus
extern "C" {
#endif
const char *myproxy_mobile_call(const char *request_json);
void myproxy_mobile_free(const char *response);
#ifdef __cplusplus
}
#endif
#endif
