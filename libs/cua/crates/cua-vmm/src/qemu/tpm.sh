# Detached supervisor: stdout carries only QEMU's PID; all helpers log to disk.
exec 3>&1
exec </dev/null >>"$CUA_QEMU_LOG" 2>&1
cleanup() {
    kill "$tpm" 2>/dev/null || :
    wait "$tpm" 2>/dev/null || :
    rm -f "$CUA_TPM_SOCKET" "$CUA_TPM_PID"
}
"$CUA_SWTPM" socket --tpm2 --tpmstate "dir=$CUA_TPM_DIR" --ctrl "type=unixio,path=$CUA_TPM_SOCKET" 3>&- >>"$CUA_TPM_LOG" 2>&1 &
tpm=$!
echo "$tpm" >"$CUA_TPM_PID"
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
n=0
while [ ! -S "$CUA_TPM_SOCKET" ]; do
    kill -0 "$tpm" 2>/dev/null || { echo 'swtpm exited before socket readiness'; exit 1; }
    n=$((n + 1))
    [ "$n" -lt 100 ] || { echo 'timed out waiting for swtpm socket'; exit 1; }
    sleep 0.1
done
"$0" "$@" 3>&- &
qemu=$!
echo "$qemu" >&3
exec 3>&-
wait "$qemu"
