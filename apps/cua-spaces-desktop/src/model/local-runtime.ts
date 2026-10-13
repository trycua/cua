/** Why New Space cannot run anything on this machine, in the words of its OS. */

const DOCKER_DESKTOP_URL = "https://docs.docker.com/desktop/setup/install/windows-install/";

/**
 * The line New Space shows when no local runtime is ready. `containerDetail`
 * is the runtime doctor's detail for the container engine, when it gave one
 * (for example "container engine at npipe://... is not running").
 */
export function noLocalRuntimeReason(platform: string, containerDetail?: string): string {
  if (platform === "darwin") return "No local runtime found (Docker or Lume).";
  const detail = (containerDetail ?? "").toLowerCase();
  const installedButStopped = /not running|did not answer|not listening|start /.test(detail);
  if (platform === "win32") {
    if (installedButStopped) {
      return "Docker Desktop isn't running. Start it and wait for “Engine running”, then try again.";
    }
    return `Cua runs local Spaces in Docker Desktop (WSL 2 alone isn't enough). Install it from ${DOCKER_DESKTOP_URL}, start it, then try again.`;
  }
  if (installedButStopped) return "Docker isn't running. Start it (for example `sudo systemctl start docker`), then try again.";
  return "No local runtime found. Install Docker Engine or Podman and start it, then try again.";
}
