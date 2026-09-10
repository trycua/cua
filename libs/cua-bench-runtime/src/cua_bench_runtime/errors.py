"""Runtime errors with stable process semantics."""

from cua_bench_runtime import exit_codes


class CbError(Exception):
    exit_code = exit_codes.HARNESS
    status = "harness_error"


class UsageFailure(CbError):
    exit_code = exit_codes.USAGE
    status = "usage_error"


class ValidationFailure(CbError):
    exit_code = exit_codes.VALIDATION
    status = "validation_error"


class HarnessFailure(CbError):
    exit_code = exit_codes.HARNESS
    status = "harness_error"


class DeadlineExceeded(CbError):
    exit_code = exit_codes.TIMEOUT
    status = "timeout"


class BudgetExceeded(CbError):
    exit_code = exit_codes.BUDGET
    status = "cost_limit"


class TrialInterrupted(CbError):
    exit_code = exit_codes.INTERRUPTED
    status = "interrupted"


class HardAbort(CbError):
    exit_code = exit_codes.HARD_ABORT
    status = "hard_abort"
