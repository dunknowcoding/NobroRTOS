"""Generated-compatible public diagnostic identities used by Python authoring."""

APP_DIAGNOSTICS = {
    "app-state": ("NOBRO-E050", "Application graph state does not allow this operation."),
    "app-name": ("NOBRO-E051", "Names must use stable lowercase labels."),
    "app-period": ("NOBRO-E052", "Task rate and period must be valid."),
    "app-task-capacity": ("NOBRO-E053", "Application task capacity is exceeded."),
    "app-wire-capacity": ("NOBRO-E054", "Application wire capacity is exceeded."),
    "app-endpoint": ("NOBRO-E055", "Wire endpoints must name existing tasks."),
    "app-duplicate-task": ("NOBRO-E056", "Task name is already declared."),
    "app-options": ("NOBRO-E057", "Task timing or resource options are invalid."),
    "app-admission": ("NOBRO-E058", "Application graph admission failed."),
    "app-step": ("NOBRO-E059", "Task callback failed."),
    "app-duplicate-wire": ("NOBRO-E060", "Wire is already declared."),
    "app-self-wire": ("NOBRO-E061", "A task cannot wire to itself."),
    "app-role": ("NOBRO-E062", "Task role is unsupported."),
    "app-target": ("NOBRO-E063", "Board or app schema is unsupported."),
    "app-empty": ("NOBRO-E064", "Application graph needs at least one task."),
    "app-shape": ("NOBRO-E065", "Application declaration shape or field type is invalid."),
}


def diagnostic(key: str) -> tuple[str, str]:
    """Return the stable code and plain-sentence summary for one app failure."""

    return APP_DIAGNOSTICS[key]
