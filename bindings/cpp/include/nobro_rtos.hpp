/*
 * NobroRTOS C++ convenience wrappers.
 *
 * Include this header after making bindings/c/include available on the include
 * path. The wrapper stays allocation-free and delegates ABI layout to C.
 */

#ifndef NOBRO_RTOS_HPP
#define NOBRO_RTOS_HPP

#include "nobro_rtos.h"

#include <cstdint>

namespace nobro {
namespace rtos {

enum class ReportStatus : std::uint8_t {
    Missing = NOBRO_REPORT_STATUS_MISSING,
    InProgress = NOBRO_REPORT_STATUS_IN_PROGRESS,
    Pass = NOBRO_REPORT_STATUS_PASS,
    Fail = NOBRO_REPORT_STATUS_FAIL,
    Corrupt = NOBRO_REPORT_STATUS_CORRUPT,
};

constexpr bool passing(ReportStatus status) noexcept {
    return status == ReportStatus::Pass;
}

class BoardProfileReportView {
public:
    constexpr explicit BoardProfileReportView(const nobro_board_profile_report_t& report) noexcept
        : report_(&report) {}

    ReportStatus status() const noexcept {
        return static_cast<ReportStatus>(nobro_board_profile_report_status(report_));
    }

    constexpr std::uint32_t board_hash() const noexcept {
        return report_->board_hash;
    }

    constexpr std::uint32_t flash_budget_bytes() const noexcept {
        return report_->flash_budget_bytes;
    }

    constexpr std::uint32_t ram_budget_bytes() const noexcept {
        return report_->ram_budget_bytes;
    }

private:
    const nobro_board_profile_report_t* report_;
};

class BoardPackageReportView {
public:
    constexpr explicit BoardPackageReportView(const nobro_board_package_report_t& report) noexcept
        : report_(&report) {}

    ReportStatus status() const noexcept {
        return static_cast<ReportStatus>(nobro_board_package_report_status(report_));
    }

    constexpr std::uint32_t boot_layout() const noexcept {
        return report_->boot_layout;
    }

    constexpr std::uint32_t app_flash_start() const noexcept {
        return report_->app_flash_start;
    }

    constexpr std::uint32_t error_code() const noexcept {
        return report_->error_code;
    }

private:
    const nobro_board_package_report_t* report_;
};

class ManifestReportView {
public:
    constexpr explicit ManifestReportView(const nobro_manifest_report_t& report) noexcept
        : report_(&report) {}

    ReportStatus status() const noexcept {
        return static_cast<ReportStatus>(nobro_manifest_report_status(report_));
    }

    constexpr std::uint32_t module_count() const noexcept {
        return report_->module_count;
    }

    constexpr std::uint32_t fingerprint() const noexcept {
        return report_->fingerprint;
    }

    constexpr std::uint32_t error_code() const noexcept {
        return report_->error_code;
    }

private:
    const nobro_manifest_report_t* report_;
};

class AdapterCompatReportView {
public:
    constexpr explicit AdapterCompatReportView(const nobro_adapter_compat_report_t& report) noexcept
        : report_(&report) {}

    ReportStatus status() const noexcept {
        return static_cast<ReportStatus>(nobro_adapter_compat_report_status(report_));
    }

    constexpr std::uint32_t adapter_count() const noexcept {
        return report_->adapter_count;
    }

    constexpr std::uint32_t required_bits() const noexcept {
        return report_->required_bits;
    }

    constexpr std::uint32_t error_code() const noexcept {
        return report_->error_code;
    }

private:
    const nobro_adapter_compat_report_t* report_;
};

} // namespace rtos
} // namespace nobro

#endif /* NOBRO_RTOS_HPP */
