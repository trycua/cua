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

export const SANDBOX_PRESETS: SandboxPreset[] = [
  {
    id: 'xfce',
    name: 'Linux (XFCE)',
    description: 'Lightweight Linux desktop with XFCE window manager',
    image: 'trycua/cua-xfce:latest',
    vncPort: 6901,
    apiPort: 8000,
    vncPath: '/vnc.html?resize=scale&autoconnect=true&quality=6',
    dockerCommand:
      'docker run --rm -it --shm-size=512m -p 6901:6901 -p 8000:8000 trycua/cua-xfce:latest',
    prerequisites: 'Docker installed and running',
    icon: 'linux',
  },
  // TODO: Uncomment these presets as they are tested and ready
  // {
  //   id: 'kasm',
  //   name: 'Linux (Kasm)',
  //   description: 'Feature-rich Linux desktop powered by Kasm Workspaces',
  //   image: 'trycua/cua-ubuntu:latest',
  //   vncPort: 6901,
  //   apiPort: 8000,
  //   vncPath: '/vnc.html',
  //   dockerCommand:
  //     'docker run --rm -it --shm-size=512m -p 6901:6901 -p 8000:8000 trycua/cua-ubuntu:latest',
  //   prerequisites: 'Docker installed and running',
  //   icon: 'linux',
  // },
  // {
  //   id: 'qemu-linux',
  //   name: 'QEMU Linux',
  //   description: 'Full Linux VM running inside QEMU emulator',
  //   image: 'trycua/cua-qemu-linux:latest',
  //   vncPort: 8006,
  //   apiPort: 5000,
  //   vncPath: '',
  //   dockerCommand:
  //     'docker run --rm -it --shm-size=1g -p 8006:8006 -p 5000:5000 trycua/cua-qemu-linux:latest',
  //   prerequisites: 'Docker installed and running, KVM support recommended',
  //   icon: 'linux',
  // },
  // {
  //   id: 'qemu-windows',
  //   name: 'QEMU Windows',
  //   description: 'Windows VM running inside QEMU emulator',
  //   image: 'trycua/cua-qemu-windows:latest',
  //   vncPort: 8006,
  //   apiPort: 5000,
  //   vncPath: '',
  //   dockerCommand:
  //     'docker run --rm -it --shm-size=2g -p 8006:8006 -p 5000:5000 trycua/cua-qemu-windows:latest',
  //   prerequisites: 'Docker installed and running, KVM support recommended, 8GB+ RAM',
  //   icon: 'windows',
  // },
  // {
  //   id: 'qemu-android',
  //   name: 'QEMU Android',
  //   description: 'Android VM running inside QEMU emulator',
  //   image: 'trycua/cua-qemu-android:latest',
  //   vncPort: 8006,
  //   apiPort: 5000,
  //   vncPath: '',
  //   dockerCommand:
  //     'docker run --rm -it --shm-size=1g -p 8006:8006 -p 5000:5000 trycua/cua-qemu-android:latest',
  //   prerequisites: 'Docker installed and running, KVM support recommended',
  //   icon: 'android',
  // },
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
