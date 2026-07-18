#ifndef NOBRO_NIUS_IMU_H
#define NOBRO_NIUS_IMU_H

#include <NiusIMU.h>
#include "nobro_imu.h"

namespace nobro {

/* Allocation-free bridge for every concrete NiusIMU IMUSensor. The caller
 * keeps ownership of the sensor and chooses its I2C/SPI constructor/config. */
class NiusImuAdapter {
public:
    explicit NiusImuAdapter(nimu::IMUSensor &sensor,
                            nobro_imu_family_t family = NOBRO_IMU_UNKNOWN,
                            uint8_t address = 0)
        : sensor_(sensor), family_(family), address_(address), wire_(nullptr),
          spi_(nullptr), transport_(DEFAULT_TRANSPORT), diagnostics_{} {}

    bool begin() {
        transport_ = DEFAULT_TRANSPORT;
        wire_ = nullptr;
        spi_ = nullptr;
        return finishBegin(sensor_.begin());
    }

    bool beginI2C(TwoWire &wire, uint8_t address) {
        address_ = address;
        wire_ = &wire;
        spi_ = nullptr;
        transport_ = I2C_TRANSPORT;
        return finishBegin(sensor_.beginI2C(wire, address));
    }

    bool beginSPI(SPIClass &spi, uint8_t chip_select) {
        address_ = chip_select;
        spi_ = &spi;
        wire_ = nullptr;
        transport_ = SPI_TRANSPORT;
        return finishBegin(sensor_.beginSPI(spi, chip_select));
    }

    bool sample(nobro_imu_sample_t &out) {
        if (!sensor_.update()) {
            diagnostics_.read_errors = increment(diagnostics_.read_errors);
            if (diagnostics_.consecutive_errors != UINT16_MAX)
                diagnostics_.consecutive_errors++;
            diagnostics_.last_event = NOBRO_IMU_EVENT_READ_ERROR;
            return false;
        }
        const nimu::IMUData &data = sensor_.data();
        vector(data.accel, 1000.0f, out.accel_mg);
        vector(data.gyro, 1000.0f, out.gyro_mdps);
        vector(data.mag, 1000.0f, out.mag_milli_ut);
        out.accel_mag_mg = magnitude(out.accel_mg);
        out.temperature_centi_c = scaled(data.temperature, 100.0f);
        out.timestamp_us = data.timestamp;
        diagnostics_.samples = increment(diagnostics_.samples);
        diagnostics_.consecutive_errors = 0;
        diagnostics_.last_event = NOBRO_IMU_EVENT_SAMPLE;
        return true;
    }

    nobro_imu_identity_t identity() {
        nobro_imu_identity_t value = {
            family_, sensor_.whoAmI(), address_, sensor_.hasMagnetometer()
        };
        return value;
    }

    bool recover() {
        if (diagnostics_.recovery_attempts != UINT8_MAX)
            diagnostics_.recovery_attempts++;
        bool ready = false;
        if (transport_ == I2C_TRANSPORT && wire_ != nullptr)
            ready = sensor_.beginI2C(*wire_, address_);
        else if (transport_ == SPI_TRANSPORT && spi_ != nullptr)
            ready = sensor_.beginSPI(*spi_, address_);
        else
            ready = sensor_.begin();
        if (!ready) {
            diagnostics_.last_event = NOBRO_IMU_EVENT_RECOVERY_EXHAUSTED;
            return false;
        }
        diagnostics_.recoveries = increment(diagnostics_.recoveries);
        diagnostics_.consecutive_errors = 0;
        diagnostics_.last_event = NOBRO_IMU_EVENT_RECOVERED;
        return true;
    }

    nobro_imu_diagnostics_t diagnostics() const { return diagnostics_; }

    nobro_imu_calibration_t calibration() const {
        const nimu::IMUCalibration source = sensor_.getCalibration();
        nobro_imu_calibration_t result = {};
        vector(source.accelBias, 1000.0f, result.accel_bias_mg);
        vector(source.accelScale, 1000000.0f, result.accel_scale_ppm);
        vector(source.gyroBias, 1000.0f, result.gyro_bias_mdps);
        vector(source.magBias, 1000.0f, result.mag_bias_milli_ut);
        vector(source.magScale, 1000000.0f, result.mag_scale_ppm);
        result.magic = source.magic;
        return result;
    }

    bool setCalibration(const nobro_imu_calibration_t &source) {
        if (source.magic != NOBRO_IMU_CALIBRATION_MAGIC) {
            diagnostics_.last_event = NOBRO_IMU_EVENT_CALIBRATION_REJECTED;
            return false;
        }
        nimu::IMUCalibration result;
        result.accelBias = vector(source.accel_bias_mg, 0.001f);
        result.accelScale = vector(source.accel_scale_ppm, 0.000001f);
        result.gyroBias = vector(source.gyro_bias_mdps, 0.001f);
        result.magBias = vector(source.mag_bias_milli_ut, 0.001f);
        result.magScale = vector(source.mag_scale_ppm, 0.000001f);
        result.magic = source.magic;
        sensor_.setCalibration(result);
        return true;
    }

private:
    bool finishBegin(bool ready) {
        if (!ready) {
            diagnostics_.read_errors = increment(diagnostics_.read_errors);
            if (diagnostics_.consecutive_errors != UINT16_MAX)
                diagnostics_.consecutive_errors++;
            diagnostics_.last_event = NOBRO_IMU_EVENT_READ_ERROR;
        }
        return ready;
    }
    static uint32_t increment(uint32_t value) {
        return value == UINT32_MAX ? value : value + 1u;
    }
    static int32_t scaled(float value, float scale) {
        if (value != value) return 0;
        const float converted = value * scale;
        if (converted >= 2147483647.0f) return INT32_MAX;
        if (converted <= -2147483648.0f) return INT32_MIN;
        return static_cast<int32_t>(converted);
    }
    static void vector(const nimu::Vec3 &source, float scale, int32_t out[3]) {
        out[0] = scaled(source.x, scale);
        out[1] = scaled(source.y, scale);
        out[2] = scaled(source.z, scale);
    }
    static nimu::Vec3 vector(const int32_t source[3], float scale) {
        nimu::Vec3 result;
        result.x = source[0] * scale;
        result.y = source[1] * scale;
        result.z = source[2] * scale;
        return result;
    }
    static uint32_t magnitude(const int32_t value[3]) {
        const float x = static_cast<float>(value[0]);
        const float y = static_cast<float>(value[1]);
        const float z = static_cast<float>(value[2]);
        const float result = sqrtf(x * x + y * y + z * z);
        return result >= 4294967295.0f ? UINT32_MAX : static_cast<uint32_t>(result);
    }

    nimu::IMUSensor &sensor_;
    nobro_imu_family_t family_;
    uint8_t address_;
    TwoWire *wire_;
    SPIClass *spi_;
    enum Transport : uint8_t { DEFAULT_TRANSPORT, I2C_TRANSPORT, SPI_TRANSPORT };
    Transport transport_;
    nobro_imu_diagnostics_t diagnostics_;
};

}  // namespace nobro

#endif
