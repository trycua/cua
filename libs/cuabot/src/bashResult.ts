export interface BashResult {
  stdout: string;
  stderr: string;
  exit_code: number | null;
  success: boolean;
  timed_out?: boolean;
  signal?: string;
  pid?: number;
}

export function buildContainerScript(command: string): string {
  return `#!/bin/bash
export DISPLAY=:100
export USER=user
export LOGNAME=user
export HOME=/home/user
export XDG_RUNTIME_DIR=/tmp/runtime-user
mkdir -p "$XDG_RUNTIME_DIR"
chown user:user "$XDG_RUNTIME_DIR" 2>/dev/null || true
chmod 700 "$XDG_RUNTIME_DIR" 2>/dev/null || true
if [ -f /home/user/.Xauthority ]; then
  export XAUTHORITY=/home/user/.Xauthority
fi
${command}
`;
}

export function bashResultFromExecError(err: any): BashResult {
  const exitCode = typeof err?.code === 'number' ? err.code : null;
  const signal = typeof err?.signal === 'string' ? err.signal : undefined;
  const timedOut =
    exitCode === 124 || err?.killed === true || /timed out|timeout expired/i.test(String(err?.message || ''));

  return {
    stdout: err?.stdout || '',
    stderr: err?.stderr || (timedOut ? 'Command timed out' : ''),
    exit_code: timedOut && exitCode === null ? null : exitCode,
    success: false,
    timed_out: timedOut || undefined,
    signal,
  };
}

export function exitCodeForBashResult(result: Pick<BashResult, 'success' | 'exit_code' | 'timed_out'>): number {
  if (result.success) return 0;
  if (typeof result.exit_code === 'number') return result.exit_code === 0 ? 1 : result.exit_code;
  return result.timed_out ? 124 : 1;
}
