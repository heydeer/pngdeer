// pngdeer: wasm32 has no C library, and no threads but rayon's (Web Workers, through
// wasm-bindgen-rayon). libaom's encoder runs its workers through pngdeer's interface
// (vendor/libaom/src/workers.rs), never pthread_create, and locks with these. With atomics (web/'s
// threaded build), a mutex and a condition variable are futexes on wasm's atomic wait and notify (a
// mutex as Drepper's "Futexes Are Tricky" mutex 2); without, there is one thread and the workers
// run one after another, so nothing waits: a wait could never end, so it traps.
#ifndef PNGDEER_PTHREAD_H_
#define PNGDEER_PTHREAD_H_
#include <stddef.h>

typedef struct {
  int state;  // 0 free, 1 held, 2 held and maybe waited for
} pthread_mutex_t;
typedef struct {
  int sequence;  // signals so far
} pthread_cond_t;
typedef int pthread_mutexattr_t;
typedef int pthread_condattr_t;
typedef int pthread_attr_t;
typedef int pthread_t;

static inline int pthread_mutex_init(pthread_mutex_t *m, const pthread_mutexattr_t *attr) {
  (void)attr;
  m->state = 0;
  return 0;
}
static inline int pthread_mutex_destroy(pthread_mutex_t *m) {
  (void)m;
  return 0;
}
static inline int pthread_cond_init(pthread_cond_t *c, const pthread_condattr_t *attr) {
  (void)attr;
  c->sequence = 0;
  return 0;
}
static inline int pthread_cond_destroy(pthread_cond_t *c) {
  (void)c;
  return 0;
}

#ifdef __wasm_atomics__
// takes the mutex marked as waited for, as a woken waiter can't know whether others still wait
static inline void pngdeer_lock_contended(pthread_mutex_t *m) {
  while (__atomic_exchange_n(&m->state, 2, __ATOMIC_ACQUIRE) != 0)
    __builtin_wasm_memory_atomic_wait32(&m->state, 2, -1);
}
static inline int pthread_mutex_lock(pthread_mutex_t *m) {
  int free = 0;
  if (!__atomic_compare_exchange_n(&m->state, &free, 1, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED))
    pngdeer_lock_contended(m);
  return 0;
}
static inline int pthread_mutex_unlock(pthread_mutex_t *m) {
  if (__atomic_exchange_n(&m->state, 0, __ATOMIC_RELEASE) == 2)
    __builtin_wasm_memory_atomic_notify(&m->state, 1);
  return 0;
}
// a signal between reading the sequence and waiting changes it, so the wait returns at once
static inline int pthread_cond_wait(pthread_cond_t *c, pthread_mutex_t *m) {
  int sequence = __atomic_load_n(&c->sequence, __ATOMIC_RELAXED);
  pthread_mutex_unlock(m);
  __builtin_wasm_memory_atomic_wait32(&c->sequence, sequence, -1);
  pngdeer_lock_contended(m);
  return 0;
}
static inline int pthread_cond_signal(pthread_cond_t *c) {
  __atomic_fetch_add(&c->sequence, 1, __ATOMIC_RELEASE);
  __builtin_wasm_memory_atomic_notify(&c->sequence, 1);
  return 0;
}
static inline int pthread_cond_broadcast(pthread_cond_t *c) {
  __atomic_fetch_add(&c->sequence, 1, __ATOMIC_RELEASE);
  __builtin_wasm_memory_atomic_notify(&c->sequence, (unsigned)-1);
  return 0;
}
#else
static inline int pthread_mutex_lock(pthread_mutex_t *m) {
  (void)m;
  return 0;
}
static inline int pthread_mutex_unlock(pthread_mutex_t *m) {
  (void)m;
  return 0;
}
static inline int pthread_cond_wait(pthread_cond_t *c, pthread_mutex_t *m) {
  (void)c;
  (void)m;
  __builtin_trap();
}
static inline int pthread_cond_signal(pthread_cond_t *c) {
  (void)c;
  return 0;
}
static inline int pthread_cond_broadcast(pthread_cond_t *c) {
  (void)c;
  return 0;
}
#endif

// libaom's own workers (aom_util/aom_thread.c), which pngdeer's replace: never started here
static inline int pthread_create(pthread_t *thread, const pthread_attr_t *attr, void *(*start)(void *), void *arg) {
  (void)thread;
  (void)attr;
  (void)start;
  (void)arg;
  return 1;
}
static inline int pthread_join(pthread_t thread, void **result) {
  (void)thread;
  (void)result;
  return 1;
}
static inline int pthread_attr_init(pthread_attr_t *attr) {
  *attr = 0;
  return 0;
}
static inline int pthread_attr_destroy(pthread_attr_t *attr) {
  (void)attr;
  return 0;
}
static inline int pthread_attr_getstacksize(const pthread_attr_t *attr, size_t *size) {
  (void)attr;
  *size = 1 << 20;
  return 0;
}
static inline int pthread_attr_setstacksize(pthread_attr_t *attr, size_t size) {
  (void)attr;
  (void)size;
  return 0;
}
#endif
