// pngdeer: wasm32 has no setjmp. libaom sets a jump target before it encodes and jumps to it only on
// an error, which the arguments pngdeer checks first don't reach short of running out of memory: so
// setjmp returns 0, and longjmp (libc.c) aborts, as the other libraries' errors do.
#ifndef PNGDEER_SETJMP_H_
#define PNGDEER_SETJMP_H_
typedef int jmp_buf[1];
#define setjmp(env) 0
_Noreturn void longjmp(jmp_buf env, int value);
#endif
