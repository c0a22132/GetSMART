#ifndef GETSMART_H
#define GETSMART_H

#ifdef __cplusplus
extern "C" {
#endif

char* getsmart_list_devices_json(void);
char* getsmart_get_smart_json(const char* device_id);
void getsmart_free_string(char* ptr);
const char* getsmart_version(void);

#ifdef __cplusplus
}
#endif

#endif
