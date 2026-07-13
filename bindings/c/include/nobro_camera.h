#ifndef NOBRO_CAMERA_H
#define NOBRO_CAMERA_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define NOBRO_CAMERA_API_VERSION 0x0100u

typedef enum nobro_camera_state {
    NOBRO_CAMERA_DOWN = 0,
    NOBRO_CAMERA_STARTING = 1,
    NOBRO_CAMERA_READY = 2,
    NOBRO_CAMERA_SUSPENDED = 3,
    NOBRO_CAMERA_FAULTED = 4
} nobro_camera_state_t;

typedef enum nobro_camera_pixel_format {
    NOBRO_CAMERA_JPEG = 0,
    NOBRO_CAMERA_GRAYSCALE = 1,
    NOBRO_CAMERA_RGB565 = 2,
    NOBRO_CAMERA_YUV422 = 3,
    NOBRO_CAMERA_RGB888 = 4,
    NOBRO_CAMERA_OTHER = 255
} nobro_camera_pixel_format_t;

typedef struct nobro_camera_frame_metadata {
    uint16_t width;
    uint16_t height;
    uint32_t bytes;
    uint64_t timestamp_us;
    uint8_t pixel_format;
    uint8_t reserved[7];
} nobro_camera_frame_metadata_t;

typedef struct nobro_camera_diagnostics {
    uint32_t frames_accepted;
    uint32_t frames_dropped;
    uint64_t bytes_accepted;
    uint32_t deadline_rejections;
    uint32_t memory_rejections;
    uint32_t backpressure_rejections;
    uint32_t capture_failures;
    uint32_t recoveries;
} nobro_camera_diagnostics_t;

#ifdef __cplusplus
}
#endif

#endif
