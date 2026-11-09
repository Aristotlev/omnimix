/* OmniMix universal DSP core, C ABI. Link libomnimix (.dll / .so / .dylib or static). */
#ifndef OMNIMIX_H
#define OMNIMIX_H
#include <stdbool.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

typedef struct OmxEq OmxEq;

const char *omx_version(void);

/* Interleaved float EQ. Starts enabled and flat with the 10 musical bands:
   0 Sub, 1 Bass, 2 Lows, 3 Body, 4 Mud, 5 Mids, 6 Claps, 7 Presence, 8 Hats, 9 Air. */
OmxEq *omx_eq_new(double sample_rate, uint32_t channels);
void   omx_eq_free(OmxEq *eq);

int32_t omx_eq_set_gain(OmxEq *eq, uint32_t band, float gain_db);       /* 0 ok, -1 error */
int32_t omx_eq_load_preset(OmxEq *eq, const char *name);                /* "Flat", "Bass Boost", "Punch", ... */
int32_t omx_eq_set_json(OmxEq *eq, const char *settings_json);          /* full EqSettings JSON */
char   *omx_eq_get_json(const OmxEq *eq);                               /* free with omx_string_free */
void    omx_eq_set_enabled(OmxEq *eq, bool enabled);
void    omx_eq_process(OmxEq *eq, float *interleaved, uint32_t frames); /* in place, real-time safe */
void    omx_eq_response_db(const OmxEq *eq, const double *freqs, double *out_db, uint32_t n);
void    omx_string_free(char *s);

#ifdef __cplusplus
}
#endif
#endif
