#ifndef NOBRO_MESH_H
#define NOBRO_MESH_H

#include <stdint.h>

typedef enum {
    NOBRO_MESH_DOWN = 0,
    NOBRO_MESH_ATTACHING,
    NOBRO_MESH_ATTACHED,
    NOBRO_MESH_SUSPENDED,
    NOBRO_MESH_FAULTED
} nobro_mesh_state_t;

typedef enum {
    NOBRO_MESH_OK = 0,
    NOBRO_MESH_INVALID_CONFIG,
    NOBRO_MESH_NOT_READY,
    NOBRO_MESH_TOO_LARGE,
    NOBRO_MESH_DEADLINE_MISS,
    NOBRO_MESH_PROVIDER_ERROR
} nobro_mesh_result_t;

typedef struct {
    uint32_t process_calls;
    uint32_t messages_accepted;
    uint32_t rejected;
    uint32_t deadline_misses;
    uint32_t recoveries;
} nobro_mesh_diagnostics_t;

#endif
