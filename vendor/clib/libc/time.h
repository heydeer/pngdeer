// slimdeer: wasm32 has no C library; libaom includes this header without calling into it (its
// timers are compiled out: CONFIG_OS_SUPPORT is 0).
#ifndef SLIMDEER_TIME_H_
#define SLIMDEER_TIME_H_
#endif
