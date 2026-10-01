export interface SandboxPreset {
  id: string;
  name: string;
  description: string;
  image: string;
  vncPort: number;
  apiPort: number;
  vncPath: string;
  dockerCommand: string;
  prerequisites: string;
  icon: 'linux' | 'windows' | 'android' | 'macos';
}

// Images now ship cua-spacesd (gRPC on :3211) instead of computer-server.
// TODO(cua-sdk): the playground's agent loop still needs an agent endpoint
// (POST /responses); drive these presets through @trycua/cua once it exists.
export const SANDBOX_PRESETS: SandboxPreset[] = [
  {
    id: 'xfce',
    name: 'Linux (XFCE)',
    description: 'Lightweight Linux desktop with XFCE window manager',
    image: 'trycua/cua-xfce:latest',
    vncPort: 6901,
    apiPort: 3211,
    vncPath: '/vnc.html?resize=scale&autoconnect=true&quality=6',
    dockerCommand:
      'docker run --rm -it --shm-size=512m -p 6901:6901 -p 3211:3211 trycua/cua-xfce:latest',
    prerequisites: 'Docker installed and running',
    icon: 'linux',
  },
];

/** macOS preset is informational only (not Docker-based, uses lume run) */
export const MACOS_PRESET = {
  id: 'macos-lume',
  name: 'macOS (Lume)',
  description: 'Native macOS VM via Lume (Apple Silicon only)',
  icon: 'macos' as const,
  command: 'lume run',
  note: 'Requires Lume CLI installed on macOS with Apple Silicon. Not Docker-based.',
};
