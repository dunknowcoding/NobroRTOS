#ifndef NOBRO_DISPLAY_H
#define NOBRO_DISPLAY_H

#include <stddef.h>
#include <stdint.h>

#define NOBRO_DISPLAY_RECEIPT_VERSION 1U

typedef enum {
    NOBRO_DISPLAY_DOWN = 0,
    NOBRO_DISPLAY_READY,
    NOBRO_DISPLAY_BUSY,
    NOBRO_DISPLAY_SUSPENDED,
    NOBRO_DISPLAY_FAULTED
} nobro_display_state_t;

typedef enum {
    NOBRO_DISPLAY_OK = 0,
    NOBRO_DISPLAY_INVALID_CONFIG,
    NOBRO_DISPLAY_INVALID_REGION,
    NOBRO_DISPLAY_INVALID_PAYLOAD,
    NOBRO_DISPLAY_NOT_READY,
    NOBRO_DISPLAY_BACKPRESSURED,
    NOBRO_DISPLAY_DEADLINE_MISS,
    NOBRO_DISPLAY_CANCELLED,
    NOBRO_DISPLAY_TRANSPORT_ERROR
} nobro_display_result_t;

typedef enum {
    NOBRO_DISPLAY_FRAME_QUEUED = 0,
    NOBRO_DISPLAY_FRAME_COMPLETE,
    NOBRO_DISPLAY_FRAME_CANCELLED,
    NOBRO_DISPLAY_FRAME_DEADLINE_MISS,
    NOBRO_DISPLAY_FRAME_TRANSPORT_ERROR
} nobro_display_frame_status_t;

typedef struct {
    uint16_t x, y, width, height;
} nobro_display_region_t;

typedef struct {
    uint8_t version;
    uint32_t sequence;
    nobro_display_region_t region;
    uint32_t payload_bytes;
    uint32_t payload_checksum;
    uint32_t submitted_us;
    uint32_t deadline_us;
    uint32_t completed_us;
    nobro_display_frame_status_t status;
} nobro_display_receipt_t;

#endif
