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

inline std::uint32_t stable_hash32(const char* value) noexcept {
    return nobro_stable_hash32_cstr(value);
}

class AiRouteDecisionView {
public:
    constexpr explicit AiRouteDecisionView(const nobro_ai_route_decision_t& decision) noexcept
        : decision_(&decision) {}

    constexpr std::uint8_t target() const noexcept {
        return decision_->target;
    }

    constexpr bool endpoint_circuit_open() const noexcept {
        return decision_->endpoint_circuit_open != 0;
    }

    constexpr bool uses_stale_snapshot() const noexcept {
        return decision_->uses_stale_snapshot != 0;
    }

private:
    const nobro_ai_route_decision_t* decision_;
};

inline AiRouteDecisionView decide_ai_route(
    const nobro_ai_route_policy_t& policy,
    const nobro_ai_model_contract_t& contract,
    const nobro_ai_runtime_state_t& state,
    std::uint32_t budget_us,
    nobro_ai_route_decision_t& out
) noexcept {
    out = nobro_ai_route_decide(policy, contract, state, budget_us);
    return AiRouteDecisionView(out);
}

class RosBridgeContractView {
public:
    constexpr explicit RosBridgeContractView(const nobro_ros_bridge_contract_t& contract) noexcept
        : contract_(&contract) {}

    constexpr std::uint8_t transport() const noexcept {
        return contract_->transport;
    }

    constexpr std::uint32_t bridge_id_hash() const noexcept {
        return contract_->bridge_id_hash;
    }

    constexpr std::uint32_t total_buffer_bytes() const noexcept {
        return contract_->total_buffer_bytes;
    }

    constexpr std::uint32_t max_timeout_us() const noexcept {
        return contract_->max_timeout_us;
    }

private:
    const nobro_ros_bridge_contract_t* contract_;
};

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
