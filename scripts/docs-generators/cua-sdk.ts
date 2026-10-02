#!/usr/bin/env npx tsx

/**
 * Cua SDK reference, generated from UniFFI metadata.
 *
 * Python `cua`, `@trycua/cua`, Swift `CuaSDK`, Kotlin `ai.cua.sdk` and the
 * Rust `cua-sdk` crate all expose the one API the `cua-sdk` cdylib exports,
 * so one dump describes every language. `cua-bindgen docs` (the pinned
 * uniffi_bindgen, library mode) prints it as JSON; this script renders it
 * into `docs/content/docs/cua-sdk/reference/`, one page per object (Sandbox,
 * Image, Services, Guest, Fleet, Spaces, ...) with language-tabbed
 * signatures, and writes the reference index, the Errors page, the Types
 * page (with every item) and the reference `meta.json`.
 *
 * The pages marked `spaces` (the Spaces app core, host teleport, app
 * teleport) document the Cua Spaces app export instead: the
 * `cua_spaces_ffi` namespace of the source-available (FSL-1.1-MIT)
 * `cua-spaces-ffi` cdylib, read with `cua-bindgen docs --namespace
 * cua_spaces_ffi`. It has a Swift binding only (`CuaSpacesFFI` in
 * libs/spaces-app-swift), so those pages show Swift and Rust signatures.
 *
 * Every rendered name is cross-checked against the committed bindings and
 * the Rust sources (for the app export: the Swift binding and the
 * cua-spaces-ffi source), so the naming rules cannot drift. Curated prose
 * comes from `headers/cua-sdk/<page>.md`; examples come from
 * `examples/cua-sdk/<Target>.<ext>`, which CI runs from the page (their
 * first line names the docs lane).
 *
 * Usage:
 *   pnpm --dir docs docs:generate:cua-sdk
 *   pnpm --dir docs docs:check:cua-sdk          # drift gate (CI)
 *
 * Environment:
 *   CUA_SDK_API_JSON          use this dump instead of building (tests, debugging)
 *   CUA_SDK_LIBRARY           the built libcua_sdk to read (skips `cargo build -p cua-sdk`)
 *   CUA_SPACES_FFI_API_JSON   the app export's dump (`--namespace cua_spaces_ffi`)
 *   CUA_SPACES_FFI_LIBRARY    the built libcua_spaces_ffi to read (skips `cargo build -p cua-spaces-ffi`)
 *   CUA_BINDGEN               the built cua-bindgen binary (skips building it)
 *   CARGO                     cargo executable
 */

import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import {
  REPO_ROOT,
  DOCS_CONTENT,
  EXAMPLES_DIR,
  codeFence,
  escapeMdxText,
  escapeTableCell,
  finish,
  isCheckMode,
  loadExamples,
  metaJson,
  readHeader,
  renderExamples,
  renderPage,
  slug,
  syncFiles,
  tabs,
  wordCount,
  type Example,
} from './lib/mdx';

// ============================================================================
// Dump schema (cua-bindgen docs)
// ============================================================================

export type SdkType =
  | { kind: 'u8' | 'i8' | 'u16' | 'i16' | 'u32' | 'i32' | 'u64' | 'i64' | 'f32' | 'f64' }
  | { kind: 'bool' | 'string' | 'bytes' | 'timestamp' | 'duration' }
  | { kind: 'object' | 'record' | 'enum' | 'callback'; name: string }
  | { kind: 'optional' | 'sequence'; inner: SdkType }
  | { kind: 'map'; key: SdkType; value: SdkType }
  | { kind: 'custom'; name: string; builtin: SdkType };

export type SdkDefault =
  | { kind: 'default' | 'none' | 'empty_sequence' | 'empty_map' }
  | { kind: 'bool'; value: boolean }
  | { kind: 'string' | 'int' | 'float'; value: string }
  | { kind: 'enum'; value: string; type: SdkType }
  | { kind: 'some'; inner: SdkDefault };

export interface SdkArgument {
  name: string;
  type: SdkType;
  default: SdkDefault | null;
}

export interface SdkCallable {
  name: string;
  docstring: string | null;
  async: boolean;
  arguments: SdkArgument[];
  return_type: SdkType | null;
  throws: SdkType | null;
  primary?: boolean;
  module?: string;
}

export interface SdkField {
  name: string;
  type: SdkType;
  default: SdkDefault | null;
  docstring: string | null;
}

export interface SdkObject {
  name: string;
  module: string;
  docstring: string | null;
  trait_interface: boolean;
  constructors: SdkCallable[];
  methods: SdkCallable[];
}

export interface SdkRecord {
  name: string;
  module: string;
  docstring: string | null;
  fields: SdkField[];
  methods: SdkCallable[];
}

export interface SdkEnum {
  name: string;
  module: string;
  docstring: string | null;
  error: boolean;
  flat: boolean;
  non_exhaustive: boolean;
  variants: Array<{ name: string; docstring: string | null; fields: SdkField[] }>;
  methods: SdkCallable[];
}

export interface SdkApi {
  namespace: string;
  docstring: string | null;
  objects: SdkObject[];
  records: SdkRecord[];
  enums: SdkEnum[];
  callback_interfaces: Array<{ name: string; module: string; docstring: string | null; methods: SdkCallable[] }>;
  functions: SdkCallable[];
}

// ============================================================================
// Pages
// ============================================================================

/** Methods of an object that another page documents (a large object split by task). */
export interface MemberSection {
  object: string;
  title: string;
  methods: string[];
}

export interface PageSpec {
  /** Path under reference/, without `.mdx`; `spacesd` is `spacesd/index.mdx` when `spacesd/...` pages exist. */
  slug: string;
  title: string;
  description: string;
  intro: string;
  /** Objects, records, enums and functions on this page, in order. */
  items: string[];
  /**
   * Unlisted items whose name starts with one of these land here, before
   * `modules`. The longest matching prefix of any page wins; a prefix that
   * matches nothing, or appears on two pages, fails.
   */
  prefixes?: string[];
  /** Modules whose items no page lists or prefix-matches land here. */
  modules?: string[];
  /**
   * Lists every item of this page and its subpages in a table. The Types
   * page links here instead of listing them, so a large module family stays
   * off it.
   */
  catalog?: boolean;
  /** Methods of objects documented on other pages, rendered here first. */
  members?: MemberSection[];
  /** The Python high-level (`cua_sandbox`) counterpart, when there is one. */
  python?: Array<[label: string, href: string]>;
  /**
   * The page documents the Cua Spaces app export (`cua_spaces_ffi`, FSL-1.1-MIT,
   * Swift only) instead of the SDK: its items come from that dump, and it
   * shows Swift and Rust signatures.
   */
  spaces?: boolean;
}

const PY = '/cua-sdk/reference/python';

/**
 * One page per object, grouped by task. Every exported item lands on exactly
 * one page: listed in `items`, or through its module. A module no page owns
 * fails the generator, so a new `native::*` module cannot silently drop out.
 */
export const PAGES: PageSpec[] = [
  {
    slug: 'cua',
    title: 'Cua client',
    description: 'The Cua handle: embedded or daemon client, its configuration, and the SDK version.',
    intro:
      'Every program starts from one `Cua` handle, embedded in the process or connected to a running `cua daemon`, with the same API either way. Its accessors return the other objects. The `config_*` functions read and write the user defaults in `$CUA_HOME/config.toml` (`cua config`).',
    items: ['Cua', 'CuaConfig', 'FleetSettings', 'CuaInfo', 'CuaMode', 'cua_sdk_version', 'config_list', 'config_get', 'config_set', 'config_unset', 'config_path', 'ConfigEntry', 'locations', 'LocationInfo', 'check_placement'],
    modules: ['cua', 'root'],
  },
  {
    slug: 'sandbox',
    title: 'Sandbox',
    description: 'Create, find, connect to and control sandboxes, locally, in the cloud or by address.',
    intro: '`cua.sandboxes()` returns the `Sandboxes` collection; each sandbox is a `Sandbox` handle. The same options run locally and in the cloud: `on` is the switch (`local`, `cloud`, `direct:<addr>`), `kind` and `runtime` pick the machine.',
    items: [
      'Sandboxes',
      'Sandbox',
      'SandboxInfo',
      'SandboxListing',
      'SandboxPhase',
      'SandboxStatus',
      'parse_sandbox_ref',
      'qualify_sandbox_ref',
      'ambiguous_sandbox_candidates',
      'SandboxRefParts',
    ],
    modules: ['sandbox', 'overlay'],
    python: [['`cua_sandbox.Sandbox`', `${PY}/sandbox#sandbox`]],
  },
  {
    slug: 'sandbox/lifecycle',
    title: 'Lifecycle',
    description: 'Wait for, keep alive, suspend, resume, restart, detach and delete a sandbox.',
    intro: 'Lifecycle calls on a `Sandbox` handle. They act on the container or VM, locally or in the cloud.',
    members: [
      {
        object: 'Sandbox',
        title: 'Lifecycle methods',
        methods: ['wait_ready', 'refresh', 'keep_alive', 'suspend', 'resume', 'restart', 'detach', 'delete'],
      },
    ],
    items: [],
    python: [['`Sandbox` lifecycle', `${PY}/sandbox#sandboxdestroy`]],
  },
  {
    slug: 'sandbox/without-spacesd',
    title: 'Without cua-spacesd',
    description: 'Run commands, take screenshots and open the display of a local Lume sandbox that has no cua-spacesd.',
    intro:
      'The macOS 15 image ships no cua-spacesd, so `sandbox.spacesd()` fails for it (macOS 26 runs cua-spacesd). A local Lume sandbox is still reachable: commands over `lume ssh`, screenshots and the display over the VM\'s VNC endpoint. `cua sb exec`, `cua sb screenshot` and `cua sb view` use the same calls. Other sandboxes fail with `Unsupported`.',
    members: [
      {
        object: 'Sandbox',
        title: 'Agentless methods',
        methods: ['lacks_spacesd', 'guest_sh', 'guest_screenshot', 'guest_display'],
      },
    ],
    items: ['GuestDisplay'],
    python: [['`cua_sandbox` shell and screen', `${PY}/interfaces`]],
  },
  {
    slug: 'sandbox/options',
    title: 'Create options',
    description: 'SandboxCreateOptions and what it holds: cloud options, readiness probes and sidecar containers.',
    intro:
      '`Sandboxes.create` takes one `SandboxCreateOptions`. Every field means the same thing locally and in the cloud; `on` picks where the sandbox runs, `kind` and `runtime` what runs it.',
    items: ['SandboxCreateOptions', 'CloudOptions', 'ReadinessProbe', 'Container'],
    python: [['`Sandbox.create` parameters', `${PY}/sandbox#sandboxcreate`]],
  },
  {
    slug: 'image',
    title: 'Image',
    description: 'Canonical images, image references, build layers and registry credentials.',
    intro:
      'Images are references: the functions here resolve and normalise them without pulling anything. `ImageBuild` layers and a `RegistrySecret` go into `SandboxCreateOptions`.',
    items: [
      'canonical_image',
      'image_alias',
      'normalize_image',
      'resolve_image',
      'resolve_image_with_secret',
      'ResolvedImage',
      'ImageInfo',
      'ImageBuild',
      'ImageLayer',
      'BuildFile',
      'RegistrySecret',
    ],
    modules: ['image'],
    python: [['`cua_sandbox.Image`', `${PY}/image#image`]],
  },
  {
    slug: 'services',
    title: 'Services',
    description: 'Named services, port forwards, public URLs and MCP clients of a sandbox.',
    intro:
      'Reach ports inside a sandbox: `sandbox.service(name)` for a declared service, `sandbox.forward(port)` for any port, `sandbox.public_url(...)` for a shareable URL, and `McpClient` for an MCP server. None of them needs cua-spacesd.',
    items: [
      'Service',
      'ServiceEndpoint',
      'HttpHeader',
      'HttpResponse',
      'PortForward',
      'PublicUrl',
      'McpClient',
      'McpConfig',
      'mcp_connect',
      'mcp_connect_url',
    ],
    modules: ['mcp'],
    members: [
      {
        object: 'Sandbox',
        title: 'Sandbox services',
        methods: ['service', 'services', 'forward', 'public_url', 'revoke_public_url', 'mcp_config', 'mcp'],
      },
    ],
    python: [['`cua_sandbox` services, tunnels and MCP', `${PY}/services`]],
  },
  {
    slug: 'spacesd',
    title: 'Spacesd client',
    description: 'SpacesdClient: the cua-spacesd client for processes, files, the desktop and media in a sandbox.',
    intro:
      '`sandbox.spacesd()` (or `cua.spacesd(url, token)` for any spacesd) returns a `SpacesdClient`. It picks native gRPC or gRPC-Web, sends the token and chunks transfers. Methods by task: [processes](/cua-sdk/reference/spacesd/shell), [files](/cua-sdk/reference/spacesd/files), [computer](/cua-sdk/reference/spacesd/computer) and [media](/cua-sdk/reference/spacesd/media).',
    items: ['SpacesdClient', 'SpacesdCapabilities', 'SpacesdFeature', 'SpacesdHttpRequest', 'SpacesdHttpResponse', 'SpacesdHttpHeader'],
    modules: ['spacesd', 'types'],
    python: [['`cua_sandbox` interfaces', `${PY}/interfaces`]],
  },
  {
    slug: 'spacesd/shell',
    title: 'Processes',
    description: 'Run, spawn and attach to processes in a sandbox through cua-spacesd.',
    intro: 'Run a command to completion with `run` or `sh`, or `spawn` a `SpacesdProcess` to stream its output and write its input.',
    members: [{ object: 'SpacesdClient', title: 'Run processes', methods: ['run', 'sh', 'spawn', 'attach', 'list_processes'] }],
    items: ['SpacesdProcess', 'SpacesdCommand', 'ProcessOutput', 'ExitInfo', 'ProcessEvent', 'ProcessEventKind', 'PtySize', 'ReplayMode'],
    python: [['`sb.shell`, `sb.terminal`', `${PY}/interfaces#shell`]],
  },
  {
    slug: 'spacesd/files',
    title: 'Files',
    description: 'Upload, download, list and remove files in a sandbox through cua-spacesd.',
    intro: 'Paths are guest paths unless a parameter names a local one. Transfers are chunked, so size is not capped.',
    members: [
      {
        object: 'SpacesdClient',
        title: 'File methods',
        methods: ['upload', 'upload_file', 'download', 'download_file', 'stat', 'list_dir', 'make_dir', 'remove'],
      },
    ],
    items: ['FileEntry', 'TransferResult', 'UploadOptions'],
    python: [['`sb.files`', `${PY}/interfaces#files`]],
  },
  {
    slug: 'spacesd/computer',
    title: 'Computer',
    description: 'Screenshots, pointer, keyboard, clipboard and displays of a sandbox desktop through cua-spacesd.',
    intro: 'Coordinates are screen pixels. cua-spacesd delegates input to Cua Driver in the guest; these calls need an image with a desktop.',
    members: [
      {
        object: 'SpacesdClient',
        title: 'Desktop methods',
        methods: [
          'screenshot',
          'click',
          'double_click',
          'right_click',
          'move_to',
          'drag',
          'scroll',
          'type_text',
          'press',
          'hotkey',
          'pointer_json',
          'keyboard_json',
          'get_clipboard',
          'set_clipboard',
          'cursor_position',
          'displays',
        ],
      },
    ],
    items: ['Screenshot', 'ScreenshotOptions', 'ImageFormat', 'Point'],
    python: [['`sb.mouse`, `sb.keyboard`, `sb.screen`', `${PY}/interfaces#mouse`]],
  },
  {
    slug: 'spacesd/media',
    title: 'Media',
    description: 'Video and audio streaming sessions and the sink callbacks you implement.',
    intro:
      'Media sessions deliver encoded frames and audio packets (`FrameSink`, `AudioSink`) to callbacks you implement. Decoded BGRA frames and PCM audio come with Cua Spaces: see [Decoded media](/cua-sdk/reference/spaces/decoded-media).',
    members: [
      {
        object: 'SpacesdClient',
        title: 'Open a session',
        methods: ['open_media', 'open_media_with_audio'],
      },
      { object: 'Sandbox', title: 'Media bridge', methods: ['open_media_bridge'] },
    ],
    items: ['MediaSession', 'MediaOpenOptions', 'MediaBridge'],
    modules: ['media'],
  },
  {
    slug: 'fleet',
    title: 'Fleet',
    description: 'Cloud pools and templates: apply, check, export, scale and delete (advanced).',
    intro:
      '`cua.fleet()` manages cloud capacity directly: named pools, [claims](/cua-sdk/reference/fleet/claims) and [images](/cua-sdk/reference/fleet/images). Most programs only need `Sandboxes.create` with `on: "cloud"`, which manages pools for you.',
    items: ['Fleet', 'FleetPools', 'SandboxSpec', 'PoolOptions', 'FleetPoolExport', 'FleetPool', 'FleetPoolSpec', 'FleetManagedPool', 'FleetGcReport', 'fleet_generate_claim_token'],
    python: [['`cua_sandbox.Pool`', `${PY}/pool#pool`]],
  },
  {
    slug: 'fleet/claims',
    title: 'Claims',
    description: 'Claim sandboxes from a cloud pool, keep them alive, release them, and sign service URLs.',
    intro: 'A claim binds one sandbox of a pool to you until you release it or its TTL passes.',
    members: [
      {
        object: 'Fleet',
        title: 'Claim methods',
        methods: ['claim', 'acquire', 'acquire_with', 'attach_claim', 'list_claims', 'keep_alive', 'release', 'service_url', 'create_signed_service_url'],
      },
    ],
    items: ['FleetClaim', 'FleetClaimOptions', 'FleetSandbox', 'FleetSignedUrl'],
    python: [['`Pool.claim`', `${PY}/pool#poolclaim`]],
  },
  {
    slug: 'fleet/images',
    title: 'Cloud images and runtimes',
    description: 'Fleet image resources and the runtime (gVisor or KubeVirt) each image runs on.',
    intro: 'Which cloud runtime an image needs, and the Fleet image resources of a namespace.',
    members: [{ object: 'Fleet', title: 'Image methods', methods: ['create_image', 'get_image', 'list_images', 'delete_image'] }],
    items: ['fleet_resolve_runtime', 'fleet_check_runtime', 'fleet_image_variant'],
    modules: ['fleet'],
  },
  {
    slug: 'local',
    title: 'Local runtimes',
    description: 'The container and VM runtimes this machine can host, their setup, and local images.',
    intro: '`cua.local()` checks and sets up the local runtimes and moves images in and out of the local engines.',
    items: ['Local'],
    modules: ['local'],
    python: [['`cua_sandbox` runtimes', `${PY}/runtimes`]],
  },
  {
    slug: 'spaces',
    title: 'Spaces',
    description: 'The Spaces registry: create, add, resolve, delete and remove Spaces.',
    intro:
      '`cua.spaces()` returns the `Spaces` registry. Create a Space where `on` says (local or cloud), or add an existing machine by URL, then open a [`Space`](/cua-sdk/reference/spaces/space).',
    items: ['Spaces', 'SpaceInfo', 'SpacePowerReport', 'spaces_tool_methods', 'SpacesToolMethod'],
  },
  {
    slug: 'spaces/create',
    title: 'Create options',
    description: 'What `Spaces.create` takes and reports: options, progress, cancel and GPU acceleration.',
    intro:
      '`Spaces.create` takes `SpaceCreateOptions` and returns a `SpaceCreateResult`. A `SpaceCreateListener` receives `SpaceCreateProgress` while it runs; `Spaces.cancel_create` stops it. `GpuSupport` says whether a location offers GPU acceleration.',
    items: ['SpaceCreateOptions', 'SpaceCreateResult', 'SpaceCreateProgress', 'SpaceCreateListener', 'SpaceCancelOutcome', 'GpuSupport', 'GpuOption'],
  },
  {
    slug: 'spaces/space',
    title: 'Space',
    description: 'A connected Space: shell, files, tools, windows and the hotspot.',
    intro:
      'A `Space` is a connected machine. Every primitive checks the spacesd feature it needs first and fails with `CapabilityMissing` when the Space lacks it. Streams, teleport and agents have their own pages.',
    items: [
      'Space',
      'SpaceBashResult',
      'SpaceWriteReport',
      'SpaceTransferReport',
      'SpaceSendFileOptions',
      'SpaceSendFileReport',
      'SpaceSentFile',
      'SpaceWindow',
      'SpaceToolInfo',
      'SpaceToolResult',
      'SpaceHotspotStatus',
    ],
    modules: ['spaces'],
  },
  {
    slug: 'spaces/volume',
    title: 'Cua Volume',
    description: 'The versioned volume every Space and agent shares: files, history, grants, requests and audit.',
    intro:
      'Cua Volume methods on the `Spaces` registry. Access is checked by the runtime; pass a `DriveView` to see the drive as a persistent agent does. Grants and approvals ask the user for presence. Guide: [Cua Volume](/spaces/guides/cua-volume).',
    members: [
      {
        object: 'Spaces',
        title: 'Drive methods',
        methods: [
          'volume_ls',
          'volume_read',
          'volume_write',
          'volume_delete',
          'volume_history',
          'volume_restore',
          'volume_grant',
          'volume_revoke',
          'volume_grants',
          'volume_request_access',
          'volume_requests',
          'volume_approve',
          'volume_deny',
          'volume_audit',
        ],
      },
    ],
    items: [
      'DriveView',
      'DriveListing',
      'DriveEntry',
      'DriveFile',
      'DriveObject',
      'DriveVersion',
      'DriveGrant',
      'DriveAccessRequest',
      'DriveAudit',
      'DriveAuditEvent',
      'DriveFileSync',
    ],
  },
  {
    slug: 'spaces/volume-sync',
    title: 'Cua Volume storage and sync',
    description: 'Where the drive keeps its bytes, the volume on this machine and in Spaces, sync across devices, and the block cache.',
    intro:
      'Runtime methods of the drive on the `Spaces` registry: Settings > Storage, the mount, sync state and the cache. `volume_sync_status` also lists the volume mounted in each Space. Files: [Cua Volume](/cua-sdk/reference/spaces/volume).',
    members: [
      {
        object: 'Spaces',
        title: 'Storage, the volume, sync and the cache',
        methods: [
          'volume_storage',
          'volume_storage_set',
          'volume_mount_status',
          'volume_mount',
          'volume_unmount',
          'volume_sync_status',
          'volume_sync_events',
          'volume_sync_resolve',
          'volume_cache_stats',
          'volume_cache_set',
          'volume_cache_clear',
        ],
      },
    ],
    items: [
      'DriveStorage',
      'DriveS3Settings',
      'DriveStorageUpdate',
      'DriveStorageCheck',
      'DriveMountStatus',
      'DriveSyncStatus',
      'DrivePendingUpload',
      'DriveSpaceVolume',
      'DriveConflict',
      'DriveDevice',
      'DriveSyncEvents',
      'DriveSyncEvent',
      'DriveCacheStats',
    ],
  },
  {
    slug: 'spaces/streams',
    title: 'Streams and presence',
    description: 'Media sessions of a Space and presence (who is connected, and their cursors).',
    intro: 'Open a media session for another client, stream frames to callbacks, and join presence. `PresenceView` and the `presence_cursor_art*` functions draw the cursors the same way in every Cua UI.',
    members: [
      { object: 'Space', title: 'Space streams', methods: ['open_stream', 'attach_stream', 'stream_session', 'close_stream', 'websocket_headers', 'windows', 'join_presence'] },
    ],
    items: [
      'SpaceStreamSession',
      'SpaceStreamOptions',
      'SpaceStreamTicket',
      'SpaceStreamStats',
      'SpacePresence',
      'PresenceIdentity',
      'PresenceParticipant',
      'PresenceMember',
      'PresenceCursor',
      'PresenceEvent',
      'PresenceView',
      'PresenceDrawable',
      'PresencePoint',
      'CursorArt',
      'presence_color',
      'presence_text_color',
      'presence_now_ms',
      'presence_cursor_art',
      'presence_cursor_art_all',
      'presence_cursor_art_svg',
      'presence_conformance_json',
    ],
  },
  {
    slug: 'spaces/windows',
    title: 'Windows, icons and displays',
    description: 'A Space\'s app icons (batched, through the SDK\'s one icon cache), window previews, displays and memory and storage use.',
    intro: '`app_icons` answers a whole window list in one call: cached icons come from memory or `$CUA_HOME/cache/icons`, and every miss costs one guest round trip. `thumbnail` is the Space\'s latest small preview from the cache every client on the machine shares (`$CUA_HOME/cache/thumbnails`, kept fresh by the daemon while someone asks). `window_thumbnail` previews one window, reused for a few seconds. `displays` lists the displays with their resolution and `usage` the memory and storage use.',
    members: [
      { object: 'Space', title: 'Windows, icons and displays', methods: ['app_icons', 'app_icon', 'thumbnail', 'window_thumbnail', 'displays', 'usage'] },
    ],
    items: ['SpaceAppIcon', 'SpaceAppIconRequest', 'SpaceDisplay', 'SpaceThumbnail', 'SpaceUsage'],
  },
  {
    slug: 'spaces/agents',
    title: 'Agents in a Space',
    description: 'Start, follow, message and stop coding-agent runs inside a Space.',
    intro: 'An agent run is a coding-agent CLI started in the Space with a prompt, detached from the call that started it.',
    members: [
      { object: 'Space', title: 'Agent runs', methods: ['agent_start', 'agent_status', 'agent_events', 'agent_message', 'agent_interrupt', 'agent_list', 'agent_stop'] },
      { object: 'Spaces', title: 'Harnesses', methods: ['agent_capabilities'] },
    ],
    items: ['AgentStartReport', 'AgentRunStatus', 'AgentActionReport', 'SpaceAgentOptions'],
  },
  {
    slug: 'spaces/persistent-agents',
    title: 'Persistent agents',
    description: 'Named agents whose memory outlives their Space: homes in the Cua Volume, routines, notifications, pause and resume, access to your computers.',
    intro:
      'A persistent agent keeps its harness memory in its home in the Cua Volume (`agents/<name>/`). The daemon restores the home before each run, saves it after each turn, fires routines with or without an app open, and posts notifications. See [Persistent agents](/spaces/guides/persistent-agents).',
    members: [
      {
        object: 'Spaces',
        title: 'Persistent agents',
        methods: [
          'persistent_agent_create',
          'persistent_agents',
          'persistent_agent_remove',
          'persistent_agent_send',
          'persistent_agent_save',
          'agent_pause',
          'agent_resume',
        ],
      },
      { object: 'Spaces', title: 'Routines', methods: ['routine_add', 'routines', 'routine_remove', 'routine_set_enabled'] },
      { object: 'Spaces', title: 'Notifications', methods: ['notify_user', 'notifications', 'notifications_ack'] },
      { object: 'Spaces', title: 'Access to your computers', methods: ['computer_access_grant', 'computer_access_revoke', 'computer_access'] },
    ],
    items: [
      'PersistentAgentInfo',
      'PersistentAgentOptions',
      'AgentDelivery',
      'HomeTransfer',
      'AgentPauseReport',
      'AgentResumeReport',
      'RoutineInfo',
      'NotificationInfo',
      'ComputerGrant',
    ],
    modules: ['persistent'],
  },
  {
    slug: 'spaces/cloud',
    title: 'Your cloud',
    description: 'Sandboxes and Spaces in your own AWS, Google Cloud or Modal account: connect, check, and sweep what Cua created there.',
    intro:
      'Create a sandbox or Space with `on="aws"`, `"gcp"` or `"modal"` once the cloud is connected. Each cloud uses its own CLI sign-in; Cua stores only the profile, region, project or environment names. The guest dials out to the cua.ai relay, so nothing listens on the internet. `cloud_test` creates nothing; `cloud_sweep` deletes only what Cua tagged and recorded.',
    members: [
      {
        object: 'Spaces',
        title: 'Your cloud',
        methods: ['cloud_status', 'cloud_connect', 'cloud_test', 'cloud_disconnect', 'cloud_sweep'],
      },
    ],
    items: [
      'CloudTarget',
      'CloudStatus',
      'CloudProvider',
      'CloudKind',
      'CloudCredentials',
      'CloudResource',
      'CloudConnected',
      'CloudCheck',
      'CloudTestReport',
      'CloudDisconnected',
      'CloudSweepReport',
      'CloudSweepItem',
    ],
    modules: ['cloud'],
  },
  {
    slug: 'spaces/teleport',
    title: 'Teleport',
    description: 'Move a running app between machines, with an approval callback.',
    intro:
      'Teleport exports an app session from this machine and imports it on another. A consent callback sees the manifest first. `Space.teleport` and `Space.teleport_manifest` ask the Cua Spaces daemon, which runs teleport; an embedded runtime without Cua Spaces raises `HostCapabilityMissing`. To bring any app into a Space (install it, open files in it), see [Teleport an app](/cua-sdk/reference/spaces/teleport-apps).',
    members: [{ object: 'Space', title: 'Teleport into a Space', methods: ['teleport', 'teleport_manifest'] }],
    items: ['TeleportApprover', 'TeleportDecision', 'TeleportReceipt'],
    modules: ['teleport_types'],
  },
  {
    slug: 'spaces/decoded-media',
    title: 'Decoded media',
    description: 'Media sessions that deliver decoded BGRA frames and PCM audio.',
    intro:
      'The streaming decoder ships with Cua Spaces. `spacesd_open_media_decoded` opens a media session on a `SpacesdClient` and delivers decoded video (packed BGRA; VideoToolbox on macOS, OpenH264 elsewhere) to a `DecodedFrameSink`, requesting a keyframe whenever a reference is lost (Swift: `client.openMediaDecoded(...)`). The open source SDK delivers the encoded frames: see [Media](/cua-sdk/reference/spacesd/media).',
    items: ['spacesd_open_media_decoded', 'spacesd_open_media_decoded_with_audio', 'DecodedVideoFrame', 'PcmAudio', 'DecodedFrameSink', 'PcmSink'],
    modules: ['media_decode'],
    spaces: true,
  },
  {
    slug: 'spaces/teleport-host',
    title: 'Host teleport',
    description: 'The Teleport object of the Spaces apps: providers, manifests and sends from this machine.',
    intro:
      '[`teleport(cua)`](/cua-sdk/reference/spaces/teleport-apps#teleport) returns the `Teleport` object (Swift: `cua.teleport()`). It lists the apps whose sessions this machine can export, builds manifests and sends them into a Space; the app catalog, plans and runs are on [Teleport an app](/cua-sdk/reference/spaces/teleport-apps).',
    items: ['Teleport', 'TeleportApproval'],
    modules: ['teleport'],
    spaces: true,
  },
  {
    slug: 'spaces/teleport-apps',
    title: 'Teleport an app',
    description: 'The app catalog, plans with consent, runs, drops and window drags.',
    intro: '`Teleport.catalog` lists every app on this machine with what teleport can do (full, install only, unsupported). `plan` says what will be installed, sent and imported, with a consent item per path and secret; `run` executes an approved plan with progress. `parse_drop` and `start_window_drag` feed drag-and-drop onto a Space.',
    items: ['teleport'],
    modules: ['teleport_app', 'teleport_app::space_side'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core',
    title: 'Spaces app core',
    description: 'The view models the Cua Spaces apps share: Space list, New Space wizard, teleport review, notch and Keyvault.',
    intro:
      'The Tauri and SwiftUI Cua Spaces apps render the same state from this core, so their behaviour cannot diverge. Build your own Spaces UI on it. Each screen is a state record, a pure `*_reduce` function that applies an action or event, and a `*_view` that derives what to draw. The subpages cover one screen each; the shared formatters are on [Window, menu bar and Settings](/cua-sdk/reference/spaces/app-core/window).',
    items: ['cua_spaces_register'],
    modules: ['app_core', 'app_core_types'],
    catalog: true,
    spaces: true,
  },
  {
    slug: 'spaces/app-core/spaces',
    title: 'Space list',
    description: 'The Space list (roster), the sidebar, a Space\'s detail and its remote windows.',
    intro: 'The roster is the list of Spaces the app shows, from `Spaces.list` rows. `app_roster_reduce` applies an action; `app_sidebar` and `app_space_detail` derive the main window.',
    items: [
      'AppFactWarning',
      'app_rows_to_spaces',
      'app_status_line',
      'app_this_machine_space',
      'AppHostSummaryInput',
      'app_agent_name',
      'AppAgentStatus',
      'app_ambient_dots',
      'AppAmbientDots',
      'app_group_windows_by_app',
      'AppOpenApp',
      'AppOpenWindow',
      'AppThumbnailScene',
      'AppWindowMode',
      'AppNotice',
      'AppNoticeKind',
    ],
    prefixes: ['AppRoster', 'app_roster_', 'AppSidebar', 'app_sidebar', 'AppSpace', 'app_space', 'AppRemoteWindow'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/creates',
    title: 'Creates and deletes',
    description: 'Spaces being created or deleted: the rows they show at once, their progress and how they hand over to the registry.',
    intro:
      '`app_creates_reduce` applies a create or delete action (a start, the SDK\'s create progress, a timer tick, the result) and `app_creates_compose` adds the pending rows to the registry\'s Spaces. Progress moves by the SDK\'s fractions, else by the time each phase usually takes for the image, from the `now` the shell passes.',
    items: ['AppCreatesState', 'AppCreateAction', 'AppPendingCreate', 'AppPendingDelete'],
    prefixes: ['app_creates_'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/stream',
    title: 'Stream section and facts',
    description: 'A Space detail\'s Stream rows (desktop and windows with their icons and picture in picture) and its facts with live memory and storage use.',
    intro: '`app_stream_section` builds the Stream rows both apps draw: the Desktop with the OS mark and the display\'s resolution, then one row per window titled by the window, with the app\'s icon source and a picture-in-picture button. `app_space_detail_live` adds Memory and Storage to the facts from `Space.usage`. `app_desktop_cover` says what the preview card shows before the first frame (Connecting, a Connect button, or the Space\'s own line) and `app_thumbnail_policy` how fresh the shells keep its blurred preview.',
    items: [
      'app_space_detail_live',
      'AppSpaceUsage',
      'app_usage_refresh_ms',
      'AppPipCommand',
      'AppPipEvent',
      'app_desktop_cover',
      'AppDesktopCover',
      'AppDesktopCoverInput',
      'AppDesktopCoverKind',
      'app_thumbnail_policy',
      'AppThumbnailPolicy',
    ],
    prefixes: ['AppStream', 'app_stream_'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/new-space',
    title: 'New Space wizard',
    description: 'The New Space wizard: where it runs, image, engine, address and the create call.',
    intro: '`app_wizard_initial` starts the wizard, `app_wizard_reduce` applies an action, and `app_wizard_view` derives the screen. `app_wizard_create_args` turns a finished wizard into `Spaces.create` arguments.',
    items: [
      'AppLocation',
      'AppCloudEngine',
      'AppLocalEngine',
      'AppRuntime',
      'AppKindOption',
      'AppMenuOption',
      'AppTile',
      'AppCreatePlan',
      'AppCreateSpaceArgs',
      'AppLocalStorage',
      'AppStorageVolume',
      'AppCloudPricing',
      'AppGpuChoice',
      'AppGpuRow',
    ],
    prefixes: ['AppWizard', 'app_wizard_', 'AppAddress', 'AppStep'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/images',
    title: 'Image catalog',
    description: 'The images the New Space wizard offers: groups, sizes, the image field and its suggestions.',
    intro: 'The catalog behind the New Space image field: each `AppSandboxImage` with its group, distro, tier and download sizes, and the suggestions the field shows as you type.',
    items: ['AppSandboxImage', 'AppImageGroup', 'AppImageSizes', 'AppPlatformSize', 'AppImageFieldView', 'AppImageSuggestion', 'AppSuggestionGroup', 'AppImageDistro', 'AppImageTier'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/teleport',
    title: 'Teleport picker',
    description: 'The teleport picker and its catalog.',
    intro: 'The picker lists `Teleport.catalog` entries and drives the steps up to the review. The overlays that show a teleport in progress are on [Teleport transfer and drag](/cua-sdk/reference/spaces/app-core/teleport-transfer).',
    items: ['AppEntrySection'],
    prefixes: ['AppPicker', 'app_picker_', 'AppCatalog', 'app_catalog_'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/teleport-transfer',
    title: 'Teleport transfer and drag',
    description: 'The overlays that show a teleport in progress: the transfer overlay and drag-and-drop onto a Space.',
    intro: 'The transfer overlay follows a running teleport step by step; the drag overlay shows a file or app dragged onto a Space.',
    items: [],
    prefixes: ['AppTransfer', 'app_transfer_', 'AppDragOverlay'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/teleport-run',
    title: 'Teleport plan and run',
    description: 'The teleport plan, its consent and capabilities, and the run\'s steps.',
    intro: '`app_teleport_plan` turns a `Teleport.plan` into the consent the user sees; `AppTeleportRunEvent` and `AppTeleportRunReport` follow the run.',
    items: [],
    prefixes: ['AppTeleport', 'app_teleport_'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/teleport-review',
    title: 'Teleport review',
    description: 'The review and consent before a teleport: what to send per site, from the app or from the Keyvault, and the choice remembered for next time.',
    intro: '`AppReviewView` is the review sheet. `AppReviewToggle` and `AppReviewChoice` change what is sent, per site or per part; `app_review_remember` keeps the choice for the next teleport of the same app to the same Space, and `AppVaultSource` offers the items the Keyvault already holds, which need no Keychain prompt.',
    items: ['AppReviewView', 'AppSendSource', 'AppVaultSource', 'AppRememberedChoice', 'AppPlanStepView', 'AppInstallCuaPrompt'],
    prefixes: ['AppReview', 'app_review_', 'AppConsent'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/teleport-grid',
    title: 'Teleport picker grid',
    description: 'The teleport picker\'s tiles in both apps: tabs, app and window tiles with their icon and preview sources, and the arrow keys.',
    intro: '`app_picker_app_grid`, `app_picker_window_grid` and `app_picker_remote_grid` build each tab\'s tiles; an app tile previews its frontmost window. `app_picker_grid_step` moves the selection for the arrow keys and `app_picker_grid_tabs` names the tabs.',
    items: [],
    prefixes: ['AppPickerTile', 'AppPickerGrid', 'app_picker_app_grid', 'app_picker_window_grid', 'app_picker_remote_grid', 'app_picker_grid_'],
    spaces: true,
  },
  {
    slug: 'spaces/volume-app-core',
    title: 'App core: Cua Volume storage, mount and sync',
    description: 'Settings, Storage (this machine or an S3-compatible bucket, the Finder volume, the cache), the first run\'s Cua Volume page and the Drive page\'s devices and conflicts.',
    intro:
      '`app_storage_reduce` and `app_storage_section` drive Settings, Storage from the daemon\'s `volume_storage`, `volume_mount_status` and `volume_cache_stats` answers; its `request` names the tool to run (`volume_storage_set`, `volume_mount`, `volume_cache_set`, ...). S3 keys travel only in the `volume_storage_set` request. `AppDriveCard` is the first run\'s Cua Volume page (off by default), drawn over the `app_drive_mount_preview` miniature. `AppDriveSyncInput` feeds the Drive page\'s devices and conflicts.',
    items: [],
    prefixes: [
      'AppStorage',
      'app_storage_',
      'AppDriveMount',
      'app_drive_mount_',
      'AppDriveS3',
      'AppDriveStorage',
      'AppDriveCheck',
      'AppDriveCache',
      'AppDriveCard',
      'AppDriveStepRequest',
      'AppDriveDevice',
      'AppDriveConflict',
      'AppDriveSync',
      'AppConflictView',
      'app_drive_sync_',
      'app_drive_mount_from_json',
    ],
    catalog: true,
    spaces: true,
  },
  {
    slug: 'spaces/agents-app-core',
    title: 'App core: agents, drive, notifications',
    description: 'The Agents page (persistent agents, memory, routines, computer access), the Drive page and the notifications the apps post.',
    intro: '`app_agents_reduce` and `app_agents_view` drive the Agents page from the daemon\'s persistent-agent, drive and routine tools; `app_drive_*` the Drive page; `app_notifications_plan` says which feed entries to post as system notifications (each once, none of the backlog on first run) and `app_notifications_view` draws the list.',
    items: ['AppLineView', 'AppDriveEntryInput', 'AppSystemNote', 'app_agent_rows', 'app_agent_status_label', 'app_agent_subtitle'],
    prefixes: ['AppAgent', 'app_agents_', 'AppPersistentAgent', 'AppFile', 'AppOpenFile', 'AppRoutine', 'AppComputerGrant', 'AppAccessAudit', 'AppTab', 'AppDrive', 'app_drive_', 'AppNotification', 'app_notifications_'],
    catalog: true,
    spaces: true,
  },
  {
    slug: 'spaces/app-core/cloud',
    title: 'Your cloud',
    description: 'The "Connect a cloud" sheet and the New Space wizard\'s "Your cloud": providers with their found mark, Test that creates nothing, Connect, and what a connected cloud runs at what cost.',
    intro: '`app_cloud_connect_reduce` applies an action and `app_cloud_connect_view` derives the sheet; its `request` is the `cloud_test` or `cloud_connect` call to run. `app_connected_clouds_from_status_json` turns the SDK\'s `cloud_status` into the wizard\'s `clouds`.',
    items: ['AppConnectedCloud', 'AppCloudOffer', 'app_connected_clouds_from_status_json', 'app_connected_cloud_from_json', 'app_is_cloud_word'],
    prefixes: ['AppCloudConnect', 'AppCloudProvider', 'AppCloudCheck', 'AppCloudTarget', 'AppCloudField', 'app_cloud_connect_'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/drag-trigger',
    title: 'Window drag trigger',
    description: 'When dragging a window opens Teleport to Cua: move versus resize, the notch line and the dwell.',
    intro: '`app_drag_classify` tells a move from a resize by the window\'s frame. `app_drag_trigger_initial` and `app_drag_trigger_apply` run the trigger: the cursor passes a line 5 pt above the notch\'s bottom edge, or rests 0.3 s in the Teleport to Cua box. `app_drag_displays` and `app_drag_portal_displays` give the notch geometry of each display.',
    items: ['AppStillAnchor', 'AppTriggerPhase'],
    prefixes: ['AppDrag', 'app_drag_'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/experiments',
    title: 'Experiments',
    description: 'Settings > Experiments: the switches that hide Cua Volume, your own cloud and sharing until they are turned on.',
    intro: '`app_experiments_page` derives the Experiments tab from the saved `AppExperiments`; `app_experiments_choose` applies a switch. Off only hides a feature: a mounted Volume, a connected cloud and existing shares stay as they are.',
    items: ['app_experiments_from_json', 'app_settings_with_storage', 'app_telemetry_experiments_changed', 'app_telemetry_experiments_on'],
    prefixes: ['AppExperiment', 'app_experiments_'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/run-on',
    title: 'Run on',
    description: 'The New Space Run on menu: This Mac, your machines that provide Spaces, and connected clouds.',
    intro: '`app_wizard_placement_options` lists where a new Space can run: This Mac, each machine that provides Spaces (greyed out with why when offline or at its limit), and, with the Your cloud experiment on, each connected cloud.',
    items: ['app_wizard_placement_options', 'app_space_host', 'AppPlacementOption', 'AppSpaceHost'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/power',
    title: 'Power button',
    description: 'The power button on a Space: suspend and resume, or turn off and on.',
    intro: '`app_power_button` derives the button for a Space from what its provider supports and any power change in flight (`app_creates_is_powering`).',
    items: ['app_creates_is_powering'],
    prefixes: ['AppPower', 'app_power_', 'AppSpacePower', 'AppPendingPower'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/notch',
    title: 'Notch',
    description: 'The notch switcher: state, events, layout on the screen and motion.',
    intro: 'The notch is the Space switcher at the top of the screen. `app_notch_reduce` applies an event; `app_notch_layout`, `app_notch_radii` and `app_notch_motion` place and animate it.',
    items: ['AppScreenFacts', 'AppLogicalRect', 'app_presence_name', 'app_presence_principal_id', 'app_os_icon_svg', 'app_os_icon_system_symbol'],
    prefixes: ['AppNotch', 'app_notch_'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/keyvault',
    title: 'Keyvault',
    description: 'The Keyvault broker client and its overview: items, pending requests, grants, rules and audit.',
    intro: '`KeyvaultClient` talks to the Keyvault broker and returns a `KeyvaultOverview`. The screens derived from it are on [Keyvault screens](/cua-sdk/reference/spaces/app-core/keyvault-screens).',
    items: ['AppSensitiveOption', 'AppSensitiveGroup', 'app_sensitive_group_sdk', ],
    prefixes: ['Keyvault', 'Kv', 'kv_'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/keyvault-screens',
    title: 'Keyvault screens',
    description: 'The Keyvault page, sidebar, the vault list (apps, sites and items with their locks), the unlock prompt and the approval prompt.',
    intro: 'The `kv_*` functions derive each Keyvault screen from a `KeyvaultOverview`. `kv_vault_view` groups the vault by source app and site, with a lock on each row; `kv_vault_reduce` applies search, selection and open groups; `kv_unlock_prompt` is the "Allow unattended access?" prompt. `kv_approval_open`, `kv_approval_reduce` and `kv_approval_view` drive the approval prompt; the `*_command` functions return the `KvCommand` to send.',
    items: ['KvAccessRow', 'KvAccessKind', 'KvRecentRow', 'KvDecision', 'KvDecisionTone', 'KvTri', 'KvSelection', 'KvSigningBadge', 'KvTone'],
    prefixes: ['kv_page', 'KvPage', 'kv_sidebar', 'KvSidebar', 'KvCategory', 'kv_list', 'KvListView', 'kv_vault_', 'KvVault', 'KvLock', 'KvKind', 'KvAppRow', 'kv_unlock_', 'KvUnlockPrompt', 'kv_delete_', 'KvDeleteConfirm', 'kv_lock_', 'kv_live_', 'KvPendingRow', 'kv_approval_', 'KvApproval'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/onboarding-settings',
    title: 'Onboarding and settings',
    description: 'First-run onboarding, its card miniatures, the settings file and the hotkey label.',
    intro: '`app_onboarding_reduce` steps through first-run onboarding. `app_settings_load` and `app_settings_save` read and write the apps\' settings file.',
    items: ['app_format_hotkey', 'AppKeyCombo', 'AppSwitcherTheme', 'AppUsageToggle', 'AppModeChoice'],
    prefixes: ['AppOnboarding', 'app_onboarding_', 'AppSettings', 'app_settings_', 'AppPresentation', 'app_presentation_', 'AppMenuPreview', 'AppPreview', 'AppDriver', 'app_driver_'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/about',
    title: 'About, updates and launch',
    description: 'The About window and its update channel, what the apps do once they launch (the bundled cua install), and the daemon and refresh reports they read.',
    intro: '`app_about_view` builds the About window from the shell\'s `AppAboutInput`; `app_about_after_launch` and `app_about_restart_daemon` say what to do after an update or when the daemon is from another build. `AppCliInstaller` puts the bundled `cua` on PATH on first launch.',
    items: ['AppDaemonCheck', 'app_daemon_check_from_json', 'AppRefreshReport', 'app_refresh_report_from_json', 'AppUpdateChannel'],
    prefixes: ['AppAbout', 'app_about_', 'AppLaunch', 'app_launch_', 'AppCliInstall', 'AppInstallMethod'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/billing',
    title: 'Cua Cloud billing',
    description: 'The Billing row in Settings and the out-of-credit notice on a refused cloud Space.',
    intro:
      'The apps take no payment details: the website handles credit and cards. `app_billing_status` turns `Fleet.billing_status` into the core\'s `AppBillingStatus`, which Settings shows as the credit left and "Manage billing" (the website billing page). A cloud Space that Fleet refuses for want of credit (`CuaError::CloudCreditExhausted`) shows `AppCreditNotice`: one line and "Add credit", which opens the same page. Local Spaces never depend on it.',
    items: ['AppCreditNotice'],
    prefixes: ['AppBilling', 'app_billing_'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/this-machine',
    title: 'This machine',
    description: 'The This machine roster entry, its page and the host setup form.',
    intro: '`app_with_this_machine` puts This machine first in the roster. `app_host_panel` derives its page from `app_host_state`; `app_host_form_reduce` and `app_host_form_view` drive the host setup form, whose `request` goes to `app_host_setup` (Swift: `host.setupRequest(...)`).',
    items: ['app_with_this_machine', 'AppPermissionRow'],
    prefixes: ['AppHost', 'app_host_'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/window',
    title: 'Window, menu bar and Settings',
    description: 'The window chrome, a Space\'s toolbar and sections, the menu bar menu, Settings and the coding agents\' rows.',
    intro: '`app_main_chrome`, `app_menu_bar` and `app_settings_page` give the window chrome, the menu bar menu and the Settings page. `app_space_detail_copy` holds the words of a Space\'s sections; the agent functions turn `AgentSetup` results into Settings rows. `app_login_item_launch_plan` decides when the apps turn on launch at login by themselves.',
    items: [
      'app_display_name',
      'app_display_path',
      'app_format_bytes',
      'AppFact',
      'AppFactCopy',
      'app_delete_failed_text',
      'AppDetailCopy',
      'app_space_detail_copy',
      'AppDetailAction',
      'AppDetailActionId',
      'AppDeleteConfirm',
      'AppSentFileInfo',
      'app_sent_file',
      'app_sent_files_from_json',
      'app_drop_sending_text',
      'app_drop_sent_text',
      'AppSignInPhase',
      'AppTelemetryInput',
      'app_menu',
      'AppMenuInput',
      'app_openable_count',
    ],
    prefixes: [
      'AppChromeInput',
      'app_chrome_',
      'AppMainChrome',
      'app_main_chrome',
      'AppMenuItem',
      'app_menu_bar',
      'AppSettingsInput',
      'AppSettingsPage',
      'AppSettingsRow',
      'AppSettingsSection',
      'AppSettingsOption',
      'app_settings_page',
      'app_settings_input_',
      'AppAgentSetup',
      'AppAgentSettingsRow',
      'app_agent_setup',
      'app_agent_settings_',
      'AppLoginItem',
      'app_login_item_',
    ],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/devices',
    title: 'Devices',
    description: 'Settings > Devices: this device\'s enrollment, the account\'s devices, Recent Access, and the enroll and approval sheets.',
    intro:
      '`app_devices_view` derives the Devices page from `app_devices_input` (a `Devices.snapshot`): this device\'s enrollment, the account\'s devices with their buttons, approval prompts that ask for presence, and Recent Access in words. `app_enroll_reduce` / `app_enroll_view` drive the enroll sheet (sign in again, or a one-time code approved elsewhere); `app_approve_reduce` / `app_approve_view` drive the approval sheet, whose `request` goes to `Devices.approve` after Touch ID or the login password.',
    items: ['AppAuditInput', 'AppActivityRow', 'AppBannerTone', 'AppThisDevice', 'AppEnrollmentKind', 'AppApprovalPrompt', 'AppMachineInput', 'AppUnconfirmedMachine'],
    prefixes: ['AppDevice', 'app_devices_', 'AppEnroll', 'app_enroll_', 'AppApprove', 'app_approve_'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/share',
    title: 'Share sheet',
    description: 'The Share sheet: who a Space is shared with (viewer or editor) and the share request to run.',
    intro:
      '`app_share_view` derives the Share sheet from `AppShareInput` (the Space and its `Space.shares`): one row per person with a role menu, the email field and why sharing is unavailable. `app_share_reduce` applies an action; its `request` goes to `Space.share` or `Space.unshare`, which ask for presence first.',
    items: [],
    prefixes: ['AppShare', 'app_share_', 'AppRoleOption'],
    spaces: true,
  },
  {
    slug: 'spaces/app-core/telemetry',
    title: 'Usage telemetry',
    description: 'Which anonymous usage events a Spaces app step means, and sending them.',
    intro:
      'Each `app_telemetry_*` function takes a reducer step (the state before it and the action) and returns the `AppTelemetrySignal`s it means: first-run pages, Space creates, Settings, Storage, sharing and enrollment, as fixed words only. `app_telemetry_record` sends them on the SDK\'s telemetry client, where every switch and the first-run notice apply. `app_telemetry_start` and `app_telemetry_welcome_left` hold the first events until the first run\'s Welcome page is left, with its usage-data switch deciding. What is sent: [Telemetry and privacy](/cua-sdk/concepts/telemetry).',
    items: [],
    prefixes: ['AppTelemetrySignal', 'app_telemetry_'],
    spaces: true,
  },
  {
    slug: 'spaces/devices',
    title: 'Devices',
    description: 'This device as a client of the account on the relay: enroll, approve, rename, revoke and the audit log.',
    intro:
      '`auth.devices(relay_url, name)` returns this device on the relay, its key in the session\'s credential store (shared with the `cua` CLI and daemon). A device lists and reaches the account\'s machines once enrolled: right after a fresh sign-in (a sign-in through `Auth.begin_login` re-registers a device that already has a key), or by a one-time code approved from an enrolled device (`cua devices approve`). A new key of the same machine replaces its old record. Enrollment lasts 30 days, then one approval or a fresh sign-in re-verifies it. Nothing here prompts; gate `approve` behind presence in your UI.',
    items: ['Devices', 'DevicesSnapshot', 'RelayDevice', 'RelayAuditEvent', 'DeviceEnrollment'],
  },
  {
    slug: 'spaces/host',
    title: 'Host and relay',
    description: 'Unattended host setup and the cua-relay directory client.',
    intro: 'Set this machine up for unattended access, and reach account machines through the relay.',
    items: ['Host', 'Relay'],
    modules: ['host'],
  },
  {
    slug: 'auth',
    title: 'Auth',
    description: 'Sign-in and the credential store the SDK, the CLI and the daemon share.',
    intro: '`cua.auth()` signs in and reads the shared session. Cloud calls use it when no Fleet token or client credentials are configured.',
    items: ['Auth', 'LoginAttempt'],
    modules: ['auth'],
    python: [['`cua_sandbox.configure()`', `${PY}/configuration#configure`]],
  },
  {
    slug: 'agents',
    title: 'Coding agents',
    description: 'Run coding agents (Claude Code, Codex, Gemini CLI, OpenCode, ...) inside any sandbox.',
    intro: '`sandbox.agents()` (or `guest.agents()`) runs a harness over the Agent Client Protocol: normalized events, follow-ups, interrupts and results. Runs outlive the handle that started them.',
    members: [
      { object: 'Sandbox', title: 'Agents in a sandbox', methods: ['agents'] },
      { object: 'SpacesdClient', title: 'Agents through a spacesd client', methods: ['agents'] },
    ],
    items: ['Agents', 'AgentRun', 'AgentRunOptions', 'AgentRunMcpServer', 'AgentFile', 'AgentEvent', 'AgentEventPage', 'AgentRunInfo', 'AgentRunResult', 'AgentArtifact', 'agent_harnesses'],
    modules: ['agents'],
  },
  {
    slug: 'agent-setup',
    title: 'Agent setup',
    description: 'Detect coding agents, install cua skills and configure their MCP servers.',
    intro: '`cua.agent_setup()` onboards coding agents on this machine: skills and MCP configuration.',
    items: ['AgentSetup'],
    modules: ['agent_setup'],
  },
  {
    slug: 'telemetry',
    title: 'Telemetry',
    description: 'Anonymous usage telemetry switches.',
    intro: 'What is collected and never collected: [Telemetry and privacy](/cua-sdk/concepts/telemetry). These calls read and change the same switches as `cua telemetry`; the SDK records its own events.',
    items: ['telemetry_status', 'telemetry_set_enabled', 'telemetry_show_last', 'telemetry_schema_json', 'telemetry_reset_id'],
    modules: ['telemetry'],
  },
];

/** The two pages every item links into. */
const ERRORS_SLUG = 'errors';
const TYPES_SLUG = 'types';

export const LANGS = ['Python', 'TypeScript', 'Swift', 'Kotlin', 'Rust'] as const;
export type Lang = (typeof LANGS)[number];
/** The Cua Spaces app export has a Swift binding only; Rust is its source. */
export const SPACES_LANGS: readonly Lang[] = ['Swift', 'Rust'];
const TAB_GROUP = 'sdk-lang';
const REFERENCE_DIR = path.join(DOCS_CONTENT, 'cua-sdk', 'reference');
const ROUTE = '/cua-sdk/reference';
const CUA_ROOT = path.join(REPO_ROOT, 'libs', 'cua');
const SDK_SRC = path.join(CUA_ROOT, 'crates', 'cua-sdk', 'src');
const SDK_EXAMPLES = path.join(EXAMPLES_DIR, 'cua-sdk');
const GENERATOR = 'pnpm --dir docs docs:generate:cua-sdk';
const SOURCE = 'cua-bindgen docs (UniFFI metadata of libcua_sdk)';
const SPACES_SOURCE = 'cua-bindgen docs --namespace cua_spaces_ffi (UniFFI metadata of libcua_spaces_ffi)';
const SPACES_FFI_ROOT = path.join(CUA_ROOT, 'crates', 'cua-spaces-ffi');
const SPACES_SWIFT = path.join(REPO_ROOT, 'libs', 'spaces-app-swift', 'Sources', 'CuaSpacesFFI', 'CuaSpacesFFI.swift');
/** Opens every page of the Cua Spaces app export. */
export const SPACES_NOTE =
  'These APIs are part of the Cua Spaces app export for building Spaces UIs. They are source-available under FSL-1.1-MIT and ship for Swift only (`import CuaSpacesFFI`); the Rust tab shows the `cua-spaces-ffi` crate they come from. The open source SDK packages for Python, TypeScript and Kotlin do not include them.';
/** Reference pages stay readable and small (docs.cua.ai eager-imports every page). */
export const MAX_WORDS = 5000;
/** Compiled MDX size drives the docs server's memory: keep each page well under this. */
export const MAX_BYTES = 60_000;
/** docs/scripts/page-budget.test.ts: highlighted blocks and Tabs per page. */
export const MAX_FENCES = 150;
export const MAX_TABS = 30;

/** Groups of the reference sidebar; folders other generators own are listed by name. */
const NAV: Array<[label: string, pages: string[]]> = [
  ['Sandboxes', ['cua', 'sandbox', 'image', 'os-image-catalog', 'image-software', 'services', 'local', 'runtime-support']],
  ['Cloud capacity', ['fleet']],
  ['Inside a sandbox', ['spacesd', 'agents']],
  ['Spaces', ['spaces']],
  ['Account', ['auth', 'agent-setup', 'telemetry']],
  ['Errors and types', [ERRORS_SLUG, TYPES_SLUG]],
  ['Language packages', ['python', 'typescript', 'rust']],
  ['Protocol', ['protocol']],
];

// ============================================================================
// Naming (mirrors UniFFI 0.31 and uniffi-bindgen-react-native, via heck)
// ============================================================================

export function words(name: string): string[] {
  return name
    .split('_')
    .flatMap((part) => part.split(/(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])/))
    .filter(Boolean);
}

export function lowerCamel(name: string): string {
  return words(name)
    .map((w, i) => (i === 0 ? w.toLowerCase() : w[0].toUpperCase() + w.slice(1).toLowerCase()))
    .join('');
}

export function shoutySnake(name: string): string {
  return words(name)
    .map((w) => w.toUpperCase())
    .join('_');
}

/** The name an error type has in a language (Kotlin renames `*Error` to `*Exception`). */
export function errorName(lang: Lang, name: string): string {
  return lang === 'Kotlin' ? name.replace(/Error$/, 'Exception') : name;
}

/** How a variant of `e` is spelled in `lang`. */
export function variantName(lang: Lang, e: SdkEnum, variant: string): string {
  if (e.error || lang === 'Rust') return variant;
  if (lang === 'Python') return shoutySnake(variant);
  if (lang === 'Swift') return lowerCamel(variant);
  if (lang === 'Kotlin') return e.flat ? shoutySnake(variant) : variant;
  return variant;
}

export function memberName(lang: Lang, name: string): string {
  if (lang === 'Rust') return name;
  // uniffi-bindgen's Python backend prefixes keywords (`from` -> `_from`).
  if (lang === 'Python') return PYTHON_KEYWORDS.has(name) ? `_${name}` : name;
  const camel = lowerCamel(name);
  return lang === 'TypeScript' && JS_RESERVED.has(camel) ? `${camel}_` : camel;
}

// Identifiers the generators escape with `_` (a Python prefix, a TypeScript suffix; Swift and Kotlin backtick them instead).
const PYTHON_KEYWORDS = new Set(
  'False None True and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield'.split(' ')
);
const JS_RESERVED = new Set(
  'break case catch class const continue debugger default delete do else enum export extends false finally for function if import in instanceof new null return super switch this throw true try typeof var void while with yield let static implements interface package private protected public await'.split(' ')
);

/** `[bindings.<lang>.rename]` tables from libs/cua/crates/cua-sdk/uniffi.toml, as `Owner.member` -> name. */
export type Renames = Partial<Record<Lang, Map<string, string>>>;

export function parseRenames(toml: string): Renames {
  const langs: Record<string, Lang> = { python: 'Python', swift: 'Swift', kotlin: 'Kotlin' };
  const out: Renames = {};
  let current: Map<string, string> | null = null;
  for (const line of toml.split(/\r?\n/)) {
    const section = line.match(/^\s*\[bindings\.(\w+)\.rename\]\s*$/);
    if (section) {
      const lang = langs[section[1]];
      current = lang ? (out[lang] ??= new Map()) : null;
      continue;
    }
    if (/^\s*\[/.test(line)) {
      current = null;
      continue;
    }
    const entry = line.match(/^\s*"([^"]+)"\s*=\s*"([^"]+)"/);
    if (current && entry) current.set(entry[1], entry[2]);
  }
  return out;
}

/** The member name after the language's `uniffi.toml` renames. */
export function renamed(lang: Lang, owner: string | null, name: string, renames: Renames): string {
  const key = owner ? `${owner}.${name}` : name;
  return renames[lang]?.get(key) ?? memberName(lang, name);
}

// ============================================================================
// Types
// ============================================================================

export interface TypeContext {
  traits: Set<string>;
  errors: Set<string>;
  renames: Renames;
}

/** The neutral notation of the tables (the Rust spelling, objects without `Arc`). */
export function rustType(t: SdkType): string {
  switch (t.kind) {
    case 'string':
      return 'String';
    case 'bytes':
      return 'Vec<u8>';
    case 'timestamp':
      return 'SystemTime';
    case 'duration':
      return 'Duration';
    case 'optional':
      return `Option<${rustType(t.inner)}>`;
    case 'sequence':
      return `Vec<${rustType(t.inner)}>`;
    case 'map':
      return `HashMap<${rustType(t.key)}, ${rustType(t.value)}>`;
    case 'object':
    case 'record':
    case 'enum':
    case 'callback':
    case 'custom':
      return t.name;
    default:
      return t.kind;
  }
}

export function langType(lang: Lang, t: SdkType, ctx: TypeContext): string {
  const rec = (inner: SdkType) => langType(lang, inner, ctx);
  switch (t.kind) {
    case 'u8':
    case 'i8':
    case 'u16':
    case 'i16':
    case 'u32':
    case 'i32':
    case 'u64':
    case 'i64':
      return intType(lang, t.kind);
    case 'f32':
      return { Python: 'float', TypeScript: 'number', Swift: 'Float', Kotlin: 'Float', Rust: 'f32' }[lang];
    case 'f64':
      return { Python: 'float', TypeScript: 'number', Swift: 'Double', Kotlin: 'Double', Rust: 'f64' }[lang];
    case 'bool':
      return { Python: 'bool', TypeScript: 'boolean', Swift: 'Bool', Kotlin: 'Boolean', Rust: 'bool' }[lang];
    case 'string':
      return { Python: 'str', TypeScript: 'string', Swift: 'String', Kotlin: 'String', Rust: 'String' }[lang];
    case 'bytes':
      return { Python: 'bytes', TypeScript: 'ArrayBuffer', Swift: 'Data', Kotlin: 'ByteArray', Rust: 'Vec<u8>' }[lang];
    case 'timestamp':
      return {
        Python: 'datetime.datetime',
        TypeScript: 'Date',
        Swift: 'Date',
        Kotlin: 'java.time.Instant',
        Rust: 'SystemTime',
      }[lang];
    case 'duration':
      return {
        Python: 'datetime.timedelta',
        TypeScript: 'number',
        Swift: 'TimeInterval',
        Kotlin: 'java.time.Duration',
        Rust: 'Duration',
      }[lang];
    case 'optional':
      return {
        Python: `Optional[${rec(t.inner)}]`,
        TypeScript: `${rec(t.inner)} | undefined`,
        Swift: `${rec(t.inner)}?`,
        Kotlin: `${rec(t.inner)}?`,
        Rust: `Option<${rec(t.inner)}>`,
      }[lang];
    case 'sequence':
      return {
        Python: `List[${rec(t.inner)}]`,
        TypeScript: `Array<${rec(t.inner)}>`,
        Swift: `[${rec(t.inner)}]`,
        Kotlin: `List<${rec(t.inner)}>`,
        Rust: `Vec<${rec(t.inner)}>`,
      }[lang];
    case 'map':
      return {
        Python: `dict[${rec(t.key)}, ${rec(t.value)}]`,
        TypeScript: `Map<${rec(t.key)}, ${rec(t.value)}>`,
        Swift: `[${rec(t.key)}: ${rec(t.value)}]`,
        Kotlin: `Map<${rec(t.key)}, ${rec(t.value)}>`,
        Rust: `HashMap<${rec(t.key)}, ${rec(t.value)}>`,
      }[lang];
    case 'object':
      // uniffi-bindgen-react-native passes objects by their `*Like`
      // interface; trait interfaces (implemented by the caller) keep their name.
      if (lang === 'Rust') return ctx.traits.has(t.name) ? `Arc<dyn ${t.name}>` : `Arc<${t.name}>`;
      return lang === 'TypeScript' && !ctx.traits.has(t.name) ? `${t.name}Like` : t.name;
    case 'enum':
      return ctx.errors.has(t.name) ? errorName(lang, t.name) : t.name;
    case 'record':
    case 'callback':
    case 'custom':
      return t.name;
  }
}

function intType(lang: Lang, kind: string): string {
  if (lang === 'Rust') return kind;
  if (lang === 'Python') return 'int';
  if (lang === 'TypeScript') return kind.endsWith('64') ? 'bigint' : 'number';
  const bits = kind.slice(1);
  const unsigned = kind.startsWith('u');
  if (lang === 'Swift') return `${unsigned ? 'UInt' : 'Int'}${bits}`;
  const base = { '8': 'Byte', '16': 'Short', '32': 'Int', '64': 'Long' }[bits]!;
  return `${unsigned ? 'U' : ''}${base}`;
}

/** A default value as the language spells it; `null` when it has none (and always in Rust). */
export function langDefault(lang: Lang, d: SdkDefault | null, t: SdkType): string | null {
  if (!d || lang === 'Rust') return null;
  switch (d.kind) {
    case 'none':
      return { Python: 'None', TypeScript: 'undefined', Swift: 'nil', Kotlin: 'null' }[lang];
    case 'default':
      return langDefault(lang, implicitDefault(t), t);
    case 'bool':
      return lang === 'Python' ? (d.value ? 'True' : 'False') : String(d.value);
    case 'string':
      return JSON.stringify(d.value);
    case 'int':
    case 'float':
      return lang === 'TypeScript' && /64$/.test(t.kind) ? `${d.value}n` : d.value;
    case 'empty_sequence':
      return lang === 'Kotlin' ? 'listOf()' : '[]';
    case 'empty_map':
      return { Python: '{}', TypeScript: 'new Map()', Swift: '[:]', Kotlin: 'mapOf()' }[lang];
    case 'enum':
      return `${d.type.kind === 'enum' ? (d.type as { name: string }).name : ''}.${d.value}`;
    case 'some':
      return langDefault(lang, d.inner, t.kind === 'optional' ? t.inner : t);
  }
}

function implicitDefault(t: SdkType): SdkDefault {
  switch (t.kind) {
    case 'optional':
      return { kind: 'none' };
    case 'bool':
      return { kind: 'bool', value: false };
    case 'string':
      return { kind: 'string', value: '' };
    case 'sequence':
      return { kind: 'empty_sequence' };
    case 'map':
      return { kind: 'empty_map' };
    default:
      return { kind: 'int', value: '0' };
  }
}

/** A language-neutral default for the tables. */
function neutralDefault(d: SdkDefault | null, t: SdkType): string {
  if (!d) return '';
  const v = langDefault('Swift', d, t)!;
  return v === 'nil' ? 'None' : v;
}

// ============================================================================
// Signatures
// ============================================================================

type Owner = { name: string; kind: 'object' | 'trait' | 'record' | 'enum' } | null;
type Role = 'method' | 'constructor' | 'function';

export function signature(lang: Lang, c: SdkCallable, owner: Owner, ctx: TypeContext, role: Role): string {
  const ty = (t: SdkType) => langType(lang, t, ctx);
  const ret = c.return_type ? ty(c.return_type) : null;
  const args = c.arguments.map((a) => {
    const name = memberName(lang, a.name);
    const def = langDefault(lang, a.default, a.type);
    return `${name}: ${ty(a.type)}${def ? ` = ${def}` : ''}`;
  });
  const throws = c.throws ? errorName(lang, (c.throws as { name: string }).name) : null;
  const ownerName = owner?.name ?? '';
  const name = renamed(lang, owner ? ownerName : null, c.name, ctx.renames);

  if (lang === 'Python') {
    if (role === 'constructor') {
      if (c.primary) return `${ownerName}(${args.join(', ')})`;
      return `@classmethod\ndef ${name}(cls, ${args.join(', ')}) -> ${ownerName}`.replace('(cls, )', '(cls)');
    }
    const self = role === 'method' ? ['self'] : [];
    return `${c.async ? 'async ' : ''}def ${name}(${[...self, ...args].join(', ')}) -> ${ret ?? 'None'}`;
  }
  if (lang === 'TypeScript') {
    const all = c.async ? [...args, 'asyncOpts_?: { signal: AbortSignal }'] : args;
    if (role === 'constructor') {
      if (c.primary) return `new ${ownerName}(${all.join(', ')})`;
      return `static ${name}(${all.join(', ')}): ${ty({ kind: 'object', name: ownerName })}`;
    }
    const r = ret ?? 'void';
    const prefix = role === 'function' ? 'function ' : '';
    return `${prefix}${name}(${all.join(', ')}): ${c.async ? `Promise<${r}>` : r}`;
  }
  if (lang === 'Swift') {
    const effects = `${c.async ? ' async' : ''}${throws ? ' throws' : ''}`;
    if (role === 'constructor') {
      if (c.primary) return `init(${args.join(', ')})${effects}`;
      return `static func ${name}(${args.join(', ')})${effects} -> ${ownerName}`;
    }
    return `func ${name}(${args.join(', ')})${effects}${ret ? ` -> ${ret}` : ''}`;
  }
  if (lang === 'Rust') {
    const self = role === 'method' ? ['&self'] : [];
    const value = role === 'constructor' ? `Arc<${ownerName}>` : (ret ?? '()');
    const out = throws ? `Result<${value}, ${throws}>` : value;
    const name = role === 'constructor' && c.primary ? 'new' : c.name;
    return `pub ${c.async ? 'async ' : ''}fn ${name}(${[...self, ...args].join(', ')})${out === '()' ? '' : ` -> ${out}`}`;
  }
  // Kotlin
  const annotation = throws ? `@Throws(${throws}::class)\n` : '';
  if (role === 'constructor') {
    if (c.primary) return `${annotation}constructor(${args.join(', ')})`;
    return `${annotation}fun ${ownerName}.Companion.${name}(${args.join(', ')}): ${ownerName}`;
  }
  return `${annotation}${c.async ? 'suspend ' : ''}fun ${name}(${args.join(', ')})${ret ? `: ${ret}` : ''}`;
}

const FENCE_LANG: Record<Lang, string> = {
  Python: 'python',
  TypeScript: 'ts',
  Swift: 'swift',
  Kotlin: 'kotlin',
  Rust: 'rust',
};

function signatureTabs(c: SdkCallable, owner: Owner, ctx: TypeContext, role: Role, langs: readonly Lang[] = LANGS): string {
  return tabs(
    langs.map((lang) => [lang, codeFence(FENCE_LANG[lang], signature(lang, c, owner, ctx, role))]),
    TAB_GROUP
  );
}

// ============================================================================
// Docstrings
// ============================================================================

/**
 * Rust doc comments as MDX: intra-doc links become code, headings become
 * bold lines (a page has one renderer-owned H1), fenced code is kept, and
 * everything else is escaped.
 */
export function renderDoc(doc: string | null): string {
  if (!doc) return '';
  const out: string[] = [];
  let fence: string | null = null;
  for (const raw of doc.replace(/\r\n/g, '\n').split('\n')) {
    const line = raw.replace(/\s+$/, '');
    const fenceMatch = line.match(/^\s*(```+|~~~+)(.*)$/);
    if (fence) {
      out.push(line);
      if (fenceMatch && fenceMatch[1].startsWith(fence)) fence = null;
      continue;
    }
    if (fenceMatch) {
      fence = fenceMatch[1];
      const lang = fenceMatch[2].trim().split(/[\s,]/)[0];
      // rustdoc treats an unlabelled or attribute-only fence as Rust.
      const label = !lang || ['ignore', 'no_run', 'should_panic', 'text'].includes(lang) ? (lang === 'text' ? 'text' : 'rust') : lang;
      out.push(`${fenceMatch[1]}${label}`);
      continue;
    }
    const heading = line.match(/^#{1,6}\s+(.*)$/);
    const text = heading ? `**${heading[1]}**` : line;
    out.push(escapeMdxText(intraDocLinks(text)));
  }
  if (fence) out.push(fence);
  return out.join('\n').trim();
}

function intraDocLinks(text: string): string {
  return (
    text
      // [`Type::method`] and [`Type`] (no explicit target) -> code
      .replace(/\[(`[^`]+`)\](?![(\[])/g, (_, code: string) => code.replace(/::/g, '.'))
      // [text](`Type::method`) or [text](crate::path) -> text
      .replace(/\[([^\]]+)\]\((?!https?:|\/|#)[^)]*\)/g, '$1')
  );
}

/** The first sentence of a doc comment, as one table-safe line. */
export function firstSentence(doc: string | null): string {
  if (!doc) return '';
  const para = doc.split(/\n\s*\n/)[0].replace(/\s+/g, ' ').trim();
  const m = para.match(/^(.+?[.!?])(\s+[A-Z(`*]|$)/);
  let text = intraDocLinks(m ? m[1] : para);
  if ((text.match(/`/g) ?? []).length % 2) text = text.replace(/`/g, '');
  return text;
}

// ============================================================================
// Page assignment
// ============================================================================

export type ItemKind = 'object' | 'record' | 'enum' | 'function';

/** Which library an item comes from: the cua SDK or the Cua Spaces app export. */
export type Origin = 'sdk' | 'spaces';

export interface Item {
  kind: ItemKind;
  name: string;
  module: string;
  doc: string | null;
  origin: Origin;
  object?: SdkObject;
  record?: SdkRecord;
  enum?: SdkEnum;
  fn?: SdkCallable;
}

export interface Placed {
  spec: PageSpec;
  items: Item[];
  members: Array<{ section: MemberSection; object: SdkObject; methods: SdkCallable[] }>;
}

export function allItems(api: SdkApi, origin: Origin = 'sdk'): Item[] {
  return [
    ...api.objects.map((o): Item => ({ kind: 'object', name: o.name, module: o.module, doc: o.docstring, origin, object: o })),
    ...api.records.map((r): Item => ({ kind: 'record', name: r.name, module: r.module, doc: r.docstring, origin, record: r })),
    ...api.enums
      .filter((e) => !e.error)
      .map((e): Item => ({ kind: 'enum', name: e.name, module: e.module, doc: e.docstring, origin, enum: e })),
    ...api.functions.map((f): Item => ({ kind: 'function', name: f.name, module: f.module ?? 'root', doc: f.docstring, origin, fn: f })),
  ];
}

/** The SDK and the app export as one API (for links and "returned by" across both). */
export function mergeApis(api: SdkApi, spaces: SdkApi | null | undefined): SdkApi {
  if (!spaces) return api;
  return {
    ...api,
    objects: [...api.objects, ...spaces.objects],
    records: [...api.records, ...spaces.records],
    enums: [...api.enums, ...spaces.enums],
    callback_interfaces: [...api.callback_interfaces, ...spaces.callback_interfaces],
    functions: [...api.functions, ...spaces.functions],
  };
}

/**
 * Places every item and every object method on exactly one page. Listed
 * names that do not exist, modules no page owns and methods claimed twice
 * all fail, so the page map cannot drift from the API. Items of the Cua
 * Spaces app export (`spaces`) go on the `spaces` pages only, and SDK items
 * on the others only.
 */
export function placeItems(api: SdkApi, pages: PageSpec[] = PAGES, spaces?: SdkApi | null): Map<string, Placed> {
  const spacesPages = pages.filter((p) => p.spaces);
  if (spacesPages.length && !spaces) {
    throw new Error(`pages ${spacesPages.map((p) => p.slug).join(', ')} document the Cua Spaces app export, but no cua_spaces_ffi dump was loaded`);
  }
  const sdk = placeOne(api, pages.filter((p) => !p.spaces), 'sdk');
  const app = spaces ? placeOne(spaces, spacesPages, 'spaces') : new Map<string, Placed>();
  return new Map(pages.map((p) => [p.slug, (p.spaces ? app : sdk).get(p.slug)!]));
}

const EXPORT_NAME: Record<Origin, string> = { sdk: 'cua-sdk', spaces: 'the Cua Spaces app export (cua_spaces_ffi)' };

function placeOne(api: SdkApi, pages: PageSpec[], origin: Origin): Map<string, Placed> {
  if (api.callback_interfaces.length) {
    throw new Error('callback interfaces are not rendered yet; the SDK uses trait objects instead');
  }
  const items = allItems(api, origin);
  const byName = new Map(items.map((i) => [i.name, i]));
  const out = new Map<string, Placed>(pages.map((p) => [p.slug, { spec: p, items: [], members: [] }]));
  const placed = new Set<string>();
  for (const p of pages) {
    for (const name of p.items) {
      const item = byName.get(name);
      if (!item) throw new Error(`${p.slug}: \`${name}\` is not exported by ${EXPORT_NAME[origin]}; fix PAGES in scripts/docs-generators/cua-sdk.ts.`);
      if (placed.has(name)) throw new Error(`\`${name}\` is listed on two pages`);
      placed.add(name);
      out.get(p.slug)!.items.push(item);
    }
  }
  const prefixToPage = new Map<string, string>();
  for (const p of pages) {
    for (const prefix of p.prefixes ?? []) {
      if (prefixToPage.has(prefix)) throw new Error(`prefix \`${prefix}\` is on ${prefixToPage.get(prefix)} and ${p.slug}`);
      prefixToPage.set(prefix, p.slug);
    }
  }
  const usedPrefixes = new Set<string>();
  const byPrefix = (name: string): string | undefined => {
    let best: string | undefined;
    for (const prefix of prefixToPage.keys()) {
      if (name.startsWith(prefix) && (best === undefined || prefix.length > best.length)) best = prefix;
    }
    if (best !== undefined) usedPrefixes.add(best);
    return best === undefined ? undefined : prefixToPage.get(best);
  };
  const moduleToPage = new Map<string, string>();
  for (const p of pages) for (const m of p.modules ?? []) moduleToPage.set(m, p.slug);
  for (const item of items) {
    if (placed.has(item.name)) continue;
    const slug = byPrefix(item.name) ?? moduleToPage.get(item.module);
    if (!slug) {
      throw new Error(
        `${item.name} (${EXPORT_NAME[origin]}) is declared in module \`${item.module}\`, which no reference page owns. Add it to PAGES in scripts/docs-generators/cua-sdk.ts.`
      );
    }
    out.get(slug)!.items.push(item);
  }
  const unused = [...prefixToPage].filter(([prefix]) => !usedPrefixes.has(prefix));
  if (unused.length) {
    throw new Error(`prefixes that match no unlisted item: ${unused.map(([p, s]) => `\`${p}\` (${s})`).join(', ')}; fix PAGES in scripts/docs-generators/cua-sdk.ts.`);
  }
  // Methods documented away from their object's page.
  const claimed = new Map<string, string>();
  for (const p of pages) {
    for (const section of p.members ?? []) {
      const object = api.objects.find((o) => o.name === section.object);
      if (!object) throw new Error(`${p.slug}: no object ${section.object}`);
      const methods = section.methods.map((m) => {
        const method = object.methods.find((x) => x.name === m);
        if (!method) throw new Error(`${p.slug}: ${section.object} has no method ${m}`);
        const key = `${section.object}.${m}`;
        if (claimed.has(key)) throw new Error(`${key} is claimed by ${claimed.get(key)} and ${p.slug}`);
        claimed.set(key, p.slug);
        return method;
      });
      out.get(p.slug)!.members.push({ section, object, methods });
    }
  }
  return out;
}

/** Methods an object's own page renders (those no other page claims). */
function ownMethods(o: SdkObject, pages: PageSpec[]): SdkCallable[] {
  const elsewhere = new Set(
    pages.flatMap((p) => (p.members ?? []).filter((s) => s.object === o.name).flatMap((s) => s.methods))
  );
  return o.methods.filter((m) => !elsewhere.has(m.name));
}

// ============================================================================
// Anchors and links
// ============================================================================

/** Heading text of an item: records and enums carry their kind, so `Cua.info` and `CuaInfo` never share an anchor. */
export function itemHeading(item: { kind: ItemKind; name: string }): string {
  if (item.kind === 'record') return `\`${item.name}\` record`;
  if (item.kind === 'enum') return `\`${item.name}\` enum`;
  return `\`${item.name}\``;
}

export function pageRoute(slugPath: string): string {
  return `${ROUTE}/${slugPath}`;
}

/** Name -> route#anchor for every item, plus the error type. */
export function linkMap(placed: Map<string, Placed>, api: SdkApi): Map<string, string> {
  const out = new Map<string, string>();
  for (const [page, p] of placed) {
    for (const item of p.items) out.set(item.name, `${pageRoute(page)}#${slug(itemHeading(item).replace(/`/g, ''))}`);
  }
  for (const e of api.enums.filter((x) => x.error)) out.set(e.name, pageRoute(ERRORS_SLUG));
  return out;
}

export function methodAnchor(owner: string, method: string): string {
  return slug(`${owner}.${method}`);
}

function relLink(href: string, page: string): string {
  const [target, anchor] = href.split('#');
  return target === pageRoute(page) && anchor ? `#${anchor}` : href;
}

/** The named item a type is about (`Vec<Option<Sandbox>>` -> `Sandbox`). */
function namedType(t: SdkType): string | null {
  switch (t.kind) {
    case 'optional':
    case 'sequence':
      return namedType(t.inner);
    case 'map':
      return namedType(t.value);
    case 'object':
    case 'record':
    case 'enum':
    case 'callback':
    case 'custom':
      return t.name;
    default:
      return null;
  }
}

/** A type in the neutral notation, linked to its item when it has one. */
function typeRef(t: SdkType, links: Map<string, string>, page: string): string {
  const text = `\`${rustType(t)}\``;
  const name = namedType(t);
  const href = name ? links.get(name) : undefined;
  return href ? `[${text}](${relLink(href, page)})` : text;
}

// ============================================================================
// Rendering
// ============================================================================

interface RenderContext {
  ctx: TypeContext;
  /** The API this page draws on: the SDK, or for an app export page both. */
  api: SdkApi;
  /** Every item of both libraries, for the catalogs. */
  all: Item[];
  /** The signature tabs of this page. */
  langs: readonly Lang[];
  links: Map<string, string>;
  examples: Map<string, Example[]>;
  used: Set<string>;
  page: string;
  pages: PageSpec[];
  errorVariants: Set<string>;
}

/** CuaError variants a doc comment names, as links into the Errors page. */
function mentionedErrors(doc: string | null, rc: RenderContext): string[] {
  if (!doc) return [];
  // Only code spans count: `Fleet` names a variant, "a Fleet URL" does not.
  const found = [...rc.errorVariants].filter((v) => new RegExp(`\`(CuaError(::|\\.))?${v}\``).test(doc));
  return found.sort().map((v) => `[\`${v}\`](${relLink(`${pageRoute(ERRORS_SLUG)}#${slug(v)}`, rc.page)})`);
}

function exampleBlock(target: string, rc: RenderContext): string[] {
  const list = rc.examples.get(target);
  if (!list) return [];
  rc.used.add(target);
  return ['**Example**', '', renderExamples(list, TAB_GROUP), ''];
}

/** A sync, argument-less, infallible method is an accessor: one table row instead of an entry. */
function isAccessor(owner: string, m: SdkCallable, rc: RenderContext): boolean {
  if (m.async || m.throws || m.arguments.length) return false;
  if (rc.examples.has(`${owner}.${m.name}`)) return false;
  return !LANGS.some((lang) => rc.ctx.renames[lang]?.has(`${owner}.${m.name}`));
}

function renderCallable(owner: string | null, c: SdkCallable, kind: Owner, role: Role, rc: RenderContext): string[] {
  const heading = owner ? `${owner}.${role === 'constructor' && c.primary ? 'new' : c.name}` : c.name;
  const lines = [`### \`${heading}\``, '', renderDoc(c.docstring), '', signatureTabs(c, kind, rc.ctx, role, rc.langs), ''];
  if (c.arguments.length) {
    lines.push('| Parameter | Type | Default |', '| --- | --- | --- |');
    for (const a of c.arguments) {
      const def = a.default ? neutralDefault(a.default, a.type) : '';
      lines.push(
        `| \`${a.name}\` | ${typeRef(a.type, rc.links, rc.page).replace(/\|/g, '\\|')} | ${def ? `\`${def.replace(/\|/g, '\\|')}\`` : 'required'} |`
      );
    }
    lines.push('');
  }
  const facts: string[] = [];
  const ret = role === 'constructor' ? ({ kind: 'object', name: owner! } as SdkType) : c.return_type;
  if (ret) facts.push(`**Returns** ${typeRef(ret, rc.links, rc.page)}`);
  if (c.async) facts.push('**Async**');
  if (c.throws) {
    const variants = mentionedErrors(c.docstring, rc);
    facts.push(
      `**Raises** [\`CuaError\`](${relLink(pageRoute(ERRORS_SLUG), rc.page)})${variants.length ? ` (${variants.join(', ')})` : ''}`
    );
  }
  if (facts.length) lines.push(facts.join(' · '), '');
  lines.push(...exampleBlock(owner ? `${owner}.${c.name}` : c.name, rc));
  return lines;
}

function accessorTable(owner: string, list: SdkCallable[], rc: RenderContext): string[] {
  if (!list.length) return [];
  return [
    '| Accessor | Returns | Description |',
    '| --- | --- | --- |',
    ...list.map(
      (m) =>
        `| \`${m.name}()\` | ${m.return_type ? typeRef(m.return_type, rc.links, rc.page).replace(/\|/g, '\\|') : ''} | ${escapeTableCell(intraDocLinks((m.docstring ?? '').split(/\n\s*\n/)[0]))} |`
    ),
    '',
  ];
}

function methodIndex(owner: string, list: SdkCallable[], rc: RenderContext, where: Map<string, string>): string[] {
  if (!list.length) return [];
  return [
    '| Method | Description |',
    '| --- | --- |',
    ...list.map((m) => {
      const page = where.get(m.name) ?? rc.page;
      const href = page === rc.page ? `#${methodAnchor(owner, m.name)}` : `${pageRoute(page)}#${methodAnchor(owner, m.name)}`;
      return `| [\`${m.name}\`](${href}) | ${escapeTableCell(firstSentence(m.docstring))} |`;
    }),
    '',
  ];
}

/** Callables that return `name`, as links (Stripe's "returned by"). */
function returnedBy(name: string, rc: RenderContext): string {
  const out: string[] = [];
  const add = (owner: string | null, c: SdkCallable) => {
    if (!c.return_type || namedType(c.return_type) !== name) return;
    const label = owner ? `${owner}.${c.name}` : c.name;
    const href = owner ? methodHref(owner, c.name, rc) : rc.links.get(c.name);
    out.push(href ? `[\`${label}\`](${relLink(href, rc.page)})` : `\`${label}\``);
  };
  for (const o of rc.api.objects) for (const m of o.methods) add(o.name, m);
  for (const f of rc.api.functions) add(null, f);
  return out.length > 12 ? '' : out.join(', ');
}

function methodHref(owner: string, method: string, rc: RenderContext): string | undefined {
  const object = rc.api.objects.find((o) => o.name === owner);
  if (!object) return undefined;
  for (const p of rc.pages) {
    for (const s of p.members ?? []) {
      if (s.object === owner && s.methods.includes(method)) return `${pageRoute(p.slug)}#${methodAnchor(owner, method)}`;
    }
  }
  const home = rc.links.get(owner);
  if (!home) return undefined;
  const m = object.methods.find((x) => x.name === method);
  if (m && isAccessor(owner, m, rc)) return home;
  return `${home.split('#')[0]}#${methodAnchor(owner, method)}`;
}

function renderObject(o: SdkObject, rc: RenderContext): string[] {
  const kind: Owner = { name: o.name, kind: o.trait_interface ? 'trait' : 'object' };
  const lines = [`## ${itemHeading({ kind: 'object', name: o.name })}`, '', renderDoc(o.docstring), ''];
  if (o.trait_interface) {
    const how =
      rc.langs === LANGS
        ? `subclass it in Python, implement the interface in TypeScript, the \`${o.name}\` protocol in Swift, the interface in Kotlin and the trait in Rust`
        : `implement the \`${o.name}\` protocol in Swift or the trait in Rust`;
    lines.push('<Callout>', `You implement \`${o.name}\` and pass it to the SDK (a callback interface): ${how}.`, '</Callout>', '');
  } else {
    const from = returnedBy(o.name, rc);
    if (from) lines.push(`Returned by ${from}.`, '');
  }
  const own = ownMethods(o, rc.pages);
  const accessors = own.filter((m) => isAccessor(o.name, m, rc));
  const entries = own.filter((m) => !isAccessor(o.name, m, rc));
  const where = new Map<string, string>();
  for (const p of rc.pages) for (const s of p.members ?? []) if (s.object === o.name) for (const m of s.methods) where.set(m, p.slug);
  const elsewhere = o.methods.filter((m) => where.has(m.name) && !isAccessor(o.name, m, rc));
  lines.push(...methodIndex(o.name, [...o.constructors.map((c) => (c.primary ? { ...c, name: 'new' } : c)), ...entries, ...elsewhere], rc, where));
  lines.push(...accessorTable(o.name, accessors, rc));
  lines.push(...exampleBlock(o.name, rc));
  for (const c of o.constructors) lines.push(...renderCallable(o.name, c, kind, 'constructor', rc));
  for (const m of entries) lines.push(...renderCallable(o.name, m, kind, 'method', rc));
  return lines;
}

function fieldTable(fields: SdkField[], rc: RenderContext): string[] {
  if (!fields.length) return [];
  const rows = fields.map((f) => {
    const camel = lowerCamel(f.name);
    const name = camel === f.name ? `\`${f.name}\`` : `\`${f.name}\` / \`${camel}\``;
    const def = neutralDefault(f.default, f.type);
    return `| ${name} | ${typeRef(f.type, rc.links, rc.page).replace(/\|/g, '\\|')} | ${def ? `\`${def.replace(/\|/g, '\\|')}\`` : ''} | ${escapeTableCell(intraDocLinks(f.docstring ?? ''))} |`;
  });
  return ['| Field | Type | Default | Description |', '| --- | --- | --- | --- |', ...rows, ''];
}

function renderRecord(r: SdkRecord, rc: RenderContext): string[] {
  const lines = [`## ${itemHeading({ kind: 'record', name: r.name })}`, '', renderDoc(r.docstring), ''];
  const from = returnedBy(r.name, rc);
  if (from) lines.push(`Returned by ${from}.`, '');
  lines.push(...fieldTable(r.fields, rc));
  lines.push(...exampleBlock(r.name, rc));
  for (const m of r.methods) lines.push(...renderCallable(r.name, m, { name: r.name, kind: 'record' }, 'method', rc));
  return lines;
}

function enumUsage(lang: Lang, e: SdkEnum, ctx: TypeContext): string {
  const typeName = e.error ? errorName(lang, e.name) : e.name;
  return e.variants
    .map((v) => {
      const name = variantName(lang, e, v.name);
      const fields = v.fields.map((f) => `${memberName(lang, f.name)}: ${langType(lang, f.type, ctx)}`);
      if (lang === 'Rust') {
        if (e.error) return `${typeName}::${name}(String)`;
        return fields.length ? `${typeName}::${name} { ${fields.join(', ')} }` : `${typeName}::${name}`;
      }
      if (e.error) {
        return {
          Python: `${typeName}.${name}`,
          TypeScript: `${typeName}.${name}`,
          Swift: `${typeName}.${name}(message: String)`,
          Kotlin: `${typeName}.${name}`,
        }[lang];
      }
      if (e.flat) return lang === 'Swift' ? `.${name}` : `${typeName}.${name}`;
      if (lang === 'TypeScript') {
        return fields.length ? `new ${typeName}.${name}({ ${fields.join(', ')} })` : `new ${typeName}.${name}()`;
      }
      if (lang === 'Swift') return fields.length ? `.${name}(${fields.join(', ')})` : `.${name}`;
      return fields.length ? `${typeName}.${name}(${fields.join(', ')})` : `${typeName}.${name}${lang === 'Python' ? '()' : ''}`;
    })
    .join('\n');
}

function renderEnum(e: SdkEnum, rc: RenderContext): string[] {
  const lines = [`## ${itemHeading({ kind: 'enum', name: e.name })}`, '', renderDoc(e.docstring), ''];
  if (e.non_exhaustive) lines.push('Non-exhaustive: new variants may be added in minor releases.', '');
  lines.push(tabs(rc.langs.map((lang) => [lang, codeFence(FENCE_LANG[lang], enumUsage(lang, e, rc.ctx))]), TAB_GROUP), '');
  const rows = e.variants.map((v) => {
    const fields = v.fields.map((f) => `\`${f.name}: ${rustType(f.type)}\``).join(', ');
    const doc = [escapeTableCell(intraDocLinks(v.docstring ?? '')), fields ? `Fields: ${fields}` : ''].filter(Boolean).join(' ');
    return `| \`${v.name}\` | ${doc.replace(/\|(?![^`]*`)/g, '\\|')} |`;
  });
  lines.push('| Variant | Description |', '| --- | --- |', ...rows, '');
  lines.push(...exampleBlock(e.name, rc));
  for (const m of e.methods) lines.push(...renderCallable(e.name, m, { name: e.name, kind: 'enum' }, 'method', rc));
  return lines;
}

function renderItem(item: Item, rc: RenderContext): string[] {
  switch (item.kind) {
    case 'object':
      return renderObject(item.object!, rc);
    case 'record':
      return renderRecord(item.record!, rc);
    case 'enum':
      return renderEnum(item.enum!, rc);
    case 'function': {
      const lines = renderCallable(null, item.fn!, null, 'function', rc);
      lines[0] = `## ${itemHeading(item)}`;
      return lines;
    }
  }
}

function pythonLine(spec: PageSpec): string[] {
  if (!spec.python?.length) return [];
  const links = spec.python.map(([label, href]) => `[${label}](${href})`).join(', ');
  return [`Python programs usually use the high-level API instead: ${links}.`, ''];
}

/** Items (with their links) that catalog page `slug` lists, sorted by name. */
function catalogItems(slug: string, rc: RenderContext): Array<Item & { href: string }> {
  const home = pageRoute(slug);
  return rc.all
    .map((i) => ({ ...i, href: rc.links.get(i.name) ?? '' }))
    .filter((i) => {
      const route = i.href.split('#')[0];
      return route === home || route.startsWith(`${home}/`);
    })
    .sort((a, b) => a.name.localeCompare(b.name, 'en', { sensitivity: 'base' }) || a.name.localeCompare(b.name));
}

function itemTable(items: Array<Item & { href: string }>, page: string): string[] {
  const out = ['| Item | Kind | Summary |', '| --- | --- | --- |'];
  for (const i of items) out.push(`| [\`${i.name}\`](${relLink(i.href, page)}) | ${i.kind} | ${escapeTableCell(firstSentence(i.doc))} |`);
  return out;
}

/**
 * A catalog page's compact index: one line of item links per subpage (the
 * summaries live on each item's own section), so large catalogs stay under
 * the page budget.
 */
function catalogIndex(items: Array<Item & { href: string }>, page: string, rc: RenderContext): string[] {
  const groups = new Map<string, Array<Item & { href: string }>>();
  for (const i of items) {
    const target = i.href.split('#')[0];
    if (!groups.has(target)) groups.set(target, []);
    groups.get(target)!.push(i);
  }
  const out: string[] = [];
  for (const [target, group] of groups) {
    const spec = rc.pages.find((p) => pageRoute(p.slug) === target || target.endsWith(`/${p.slug}`));
    out.push(`**${spec ? spec.title : target}**: ${group.map((i) => `[\`${i.name}\`](${relLink(i.href, page)})`).join(', ')}`, '');
  }
  return out;
}

function renderObjectPage(p: Placed, rc: RenderContext, version: string): string {
  const { spec } = p;
  const body: string[] = [...(spec.spaces ? ['<Callout title="Cua Spaces app export">', SPACES_NOTE, '</Callout>', ''] : []), spec.intro, '', ...pythonLine(spec)];
  const header = readHeader(`cua-sdk/${spec.slug}.md`, { version });
  if (header) body.push(header, '');
  for (const m of p.members) {
    const home = rc.links.get(m.object.name);
    body.push(`## ${m.section.title}`, '', `Methods of [\`${m.object.name}\`](${home ? relLink(home, spec.slug) : '#'}).`, '');
    for (const method of m.methods) {
      body.push(...renderCallable(m.object.name, method, { name: m.object.name, kind: 'object' }, 'method', rc));
    }
  }
  for (const item of p.items) body.push(...renderItem(item, rc));
  if (spec.catalog) {
    const items = catalogItems(spec.slug, rc);
    body.push('## All items', '', `${items.length} items, across this page and its subpages, grouped by page. Methods are listed on their object.`, '');
    body.push(...catalogIndex(items, spec.slug, rc), '');
  }
  const needsCallout = body.some((l) => l.includes('<Callout'));
  return renderPage({
    title: spec.title,
    description: spec.description,
    generator: GENERATOR,
    source: spec.spaces ? SPACES_SOURCE : SOURCE,
    version,
    components: needsCallout ? ['Callout', 'Tabs'] : ['Tabs'],
    body: body.join('\n'),
  });
}

// ----------------------------------------------------------------------------
// Errors
// ----------------------------------------------------------------------------

/** `#[error("...")]` message templates of `CuaError`, from its source. */
export function errorMessages(source: string, enumName = 'CuaError'): Map<string, string> {
  const out = new Map<string, string>();
  const start = source.indexOf(`pub enum ${enumName} {`);
  if (start < 0) return out;
  const body = source.slice(start, source.indexOf('\n}', start));
  let pending: string | null = null;
  for (const line of body.split('\n')) {
    const msg = line.match(/#\[error\("((?:[^"\\]|\\.)*)"\)\]/);
    if (msg) pending = msg[1];
    const variant = line.match(/^\s*([A-Z]\w*)\s*[({,]/);
    if (variant && pending !== null) {
      out.set(variant[1], pending.replace(/\{0\}|\{\w*\}/g, '<detail>'));
      pending = null;
    }
  }
  return out;
}

/** A variant's doc split into cause and fix (`Fix:` starts the fix paragraph). */
export function causeAndFix(doc: string | null): { cause: string; fix: string } {
  const text = (doc ?? '').trim();
  const at = text.search(/(^|\n\s*\n)Fix:\s*/);
  if (at < 0) return { cause: text, fix: '' };
  return {
    cause: text.slice(0, at).trim(),
    fix: text.slice(at).replace(/^\s*Fix:\s*/, '').trim(),
  };
}

/** Callables whose doc comment names `variant` (as code), as links; '' when none or too many to be useful. */
function raisedBy(variant: string, rc: RenderContext): string {
  const re = new RegExp(`\`(CuaError(::|\\.))?${variant}\``);
  const out: string[] = [];
  for (const o of rc.api.objects) {
    for (const m of [...o.constructors, ...o.methods]) {
      if (!m.docstring || !re.test(m.docstring)) continue;
      const href = methodHref(o.name, m.name, rc);
      out.push(href ? `[\`${o.name}.${m.name}\`](${relLink(href, rc.page)})` : `\`${o.name}.${m.name}\``);
    }
  }
  for (const f of rc.api.functions) {
    if (!f.docstring || !re.test(f.docstring)) continue;
    const href = rc.links.get(f.name);
    out.push(href ? `[\`${f.name}\`](${relLink(href, rc.page)})` : `\`${f.name}\``);
  }
  return out.length && out.length <= 15 ? out.join(', ') : '';
}

function renderErrorsPage(api: SdkApi, rc: RenderContext, version: string, messages: Map<string, string>): string {
  const errors = api.enums.filter((e) => e.error);
  const body: string[] = [
    'Every fallible call raises (throws) one error type, `CuaError`. Its variant says what went wrong; its message carries the detail; its doc URL links to the variant\'s entry on this page (`https://cua.ai/docs/cua-sdk/reference/errors#<variant in lower case>`).',
    '',
  ];
  const header = readHeader('cua-sdk/errors.md', { version });
  if (header) body.push(header, '');
  for (const e of errors) {
    const first = e.variants[0]?.name ?? 'NotFound';
    body.push('| Language | Type | A variant | Doc URL |', '| --- | --- | --- | --- |');
    body.push(`| Python | \`cua.${e.name}\` | \`cua.${e.name}.${first}\` | \`e.doc_url\` |`);
    body.push(`| TypeScript | \`${e.name}\` | \`${e.name}.${first}\`, tested with \`${e.name}.${first}.instanceOf(e)\` | \`e.docUrl\`, \`cuaErrorDocUrl(e)\` |`);
    body.push(`| Swift | \`${e.name}\` | \`${e.name}.${first}(message:)\` | \`e.docUrl\` |`);
    body.push(`| Kotlin | \`${errorName('Kotlin', e.name)}\` | \`${errorName('Kotlin', e.name)}.${first}\` | \`e.docUrl\` |`);
    body.push(`| Rust | \`cua_sdk::${e.name}\` | \`${e.name}::${first}(String)\` | \`e.doc_url()\` |`, '');
    body.push(...exampleBlock(e.name, rc));
    body.push('## Variants', '');
    body.push('| Variant | Message | Cause |', '| --- | --- | --- |');
    for (const v of e.variants) {
      const msg = messages.get(v.name);
      body.push(
        `| [\`${v.name}\`](#${slug(v.name)}) | ${msg ? `\`${msg.replace(/\|/g, '\\|')}\`` : ''} | ${escapeTableCell(firstSentence(causeAndFix(v.docstring).cause))} |`
      );
    }
    body.push('');
    for (const v of e.variants) {
      const { cause, fix } = causeAndFix(v.docstring);
      body.push(`## \`${v.name}\``, '');
      body.push(`**Cause** ${renderDoc(cause)}`, '');
      if (fix) body.push(`**Fix** ${renderDoc(fix)}`, '');
      const msg = messages.get(v.name);
      if (msg) body.push(`**Message** \`${msg}\``, '');
      const by = raisedBy(v.name, rc);
      if (by) body.push(`**Raised by** ${by}`, '');
      body.push(...exampleBlock(`CuaError.${v.name}`, rc));
    }
  }
  return renderPage({
    title: 'Errors',
    description: 'Every error the Cua SDK raises: its variant, message, cause and fix.',
    generator: GENERATOR,
    source: SOURCE,
    version,
    components: ['Tabs'],
    body: body.join('\n'),
  });
}

// ----------------------------------------------------------------------------
// Types
// ----------------------------------------------------------------------------

function renderTypesPage(api: SdkApi, rc: RenderContext, version: string, renames: Renames): string {
  const kotlinRenames = [...(renames.Kotlin ?? new Map())]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([from, to]) => `\`${from.split('.')[0]}.${to}\``)
    .join(', ');
  const body: string[] = [
    'Tables on these pages use one neutral notation for types (the Rust spelling). This page maps it to each language and lists the items of the SDK.',
    '',
    '## Type notation',
    '',
    '| Notation | Python | TypeScript | Swift | Kotlin | Rust |',
    '| --- | --- | --- | --- | --- | --- |',
  ];
  const ctx = rc.ctx;
  const sample: Array<[string, SdkType]> = [
    ['String', { kind: 'string' }],
    ['bool', { kind: 'bool' }],
    ['u32', { kind: 'u32' }],
    ['i64', { kind: 'i64' }],
    ['u64', { kind: 'u64' }],
    ['f64', { kind: 'f64' }],
    ['Vec<u8>', { kind: 'bytes' }],
    ['Vec<T>', { kind: 'sequence', inner: { kind: 'record', name: 'T' } }],
    ['Option<T>', { kind: 'optional', inner: { kind: 'record', name: 'T' } }],
    ['HashMap<K, V>', { kind: 'map', key: { kind: 'record', name: 'K' }, value: { kind: 'record', name: 'V' } }],
    ['SystemTime', { kind: 'timestamp' }],
    ['Duration', { kind: 'duration' }],
    ['Sandbox (an object)', { kind: 'object', name: 'Sandbox' }],
  ];
  for (const [label, t] of sample) {
    body.push(`| \`${label}\` | ${LANGS.map((l) => `\`${langType(l, t, ctx).replace(/\|/g, '\\|')}\``).join(' | ')} |`);
  }
  body.push(
    '',
    '`Option<T>` may be absent: `None`, `undefined`, `nil`, `null`. A default of `None` means the argument or field can be left out.',
    '',
    '## Names in each language',
    '',
    '| | Python | TypeScript | Swift | Kotlin | Rust |',
    '| --- | --- | --- | --- | --- | --- |',
    "| Import | `import cua` | `import { ... } from '@trycua/cua'` | `import Cua` | `import ai.cua.sdk.*` | `use cua_sdk::*;` |",
    '| Methods, arguments, fields | `snake_case` | `camelCase` | `camelCase` | `camelCase` | `snake_case` |',
    '| Enum variants | `UPPER_SNAKE` | `PascalCase` | `lowerCamel` | `UPPER_SNAKE` (plain), `PascalCase` (with fields) | `PascalCase` |',
    '| Async methods | `async def`, awaited | return a `Promise`, optional `{ signal }` | `async` | `suspend` | `async fn` |',
    '| Errors | `CuaError.<Variant>` | `CuaError.<Variant>` | `CuaError.<Variant>(message:)` | `CuaException.<Variant>` | `CuaError::<Variant>(String)` |',
    '| Object handles | the class | the class; arguments and results use the `<Name>Like` interface | the class | the class | `Arc<T>` |',
    '| Optional arguments | positional (`sb.spacesd(None)`) | `undefined` | `nil` | `null` | `None` |',
    '',
    'Renamed members: in Kotlin, objects implement `AutoCloseable`, whose `close()` frees the handle, so the async `close` is renamed (' +
      kotlinRenames +
      '). In TypeScript, reserved words get a trailing underscore (`Sandbox.delete_()`). The signature tabs show the renamed form.',
    ''
  );
  const header = readHeader('cua-sdk/types.md', { version });
  if (header) body.push(header, '');
  // Items a catalog page lists stay off this page; it links there instead.
  // So does the Cua Spaces app export: its pages list it.
  const catalogs = rc.pages.filter((p) => p.catalog && !p.spaces);
  const listed = new Set(catalogs.flatMap((c) => catalogItems(c.slug, rc).map((i) => i.name)));
  const items = allItems(api)
    .filter((i) => !listed.has(i.name))
    .map((i) => ({ ...i, href: rc.links.get(i.name) }))
    .sort((a, b) => a.name.localeCompare(b.name, 'en', { sensitivity: 'base' }) || a.name.localeCompare(b.name));
  body.push('## All items', '', `${items.length + 1} items. Methods are listed on their object.`, '');
  for (const c of catalogs) {
    body.push(`The ${catalogItems(c.slug, rc).length} items of ${c.title} are listed on [its page](${pageRoute(c.slug)}#all-items).`, '');
  }
  const spacesHomes = spacesHomePages(rc.pages);
  if (spacesHomes.length) {
    const count = rc.all.filter((i) => i.origin === 'spaces').length;
    const links = spacesHomes.map((p) => `[${p.title}](${pageRoute(p.slug)}${p.catalog ? '#all-items' : ''})`).join(', ');
    body.push(`The ${count} items of the Cua Spaces app export (source-available, FSL-1.1-MIT, Swift only) are listed on their pages: ${links}.`, '');
  }
  body.push('| Item | Kind | Summary |', '| --- | --- | --- |');
  const rows = [
    ...items.map((i) => [i.name, i.kind, i.href ?? '', firstSentence(i.doc)] as const),
    ...api.enums.filter((e) => e.error).map((e) => [e.name, 'error', pageRoute(ERRORS_SLUG), firstSentence(e.docstring)] as const),
  ].sort((a, b) => a[0].localeCompare(b[0], 'en', { sensitivity: 'base' }) || a[0].localeCompare(b[0]));
  for (const [name, kind, href, summary] of rows) {
    body.push(`| [\`${name}\`](${relLink(href, TYPES_SLUG)}) | ${kind} | ${escapeTableCell(summary)} |`);
  }
  return renderPage({
    title: 'Types',
    description: 'How the SDK types map to each language, and every object, record, enum and function in one list.',
    generator: GENERATOR,
    source: SOURCE,
    version,
    body: body.join('\n'),
  });
}

/** The app export pages that list their own items: catalogs, and pages outside a catalog's subtree. */
function spacesHomePages(pages: PageSpec[]): PageSpec[] {
  const catalogs = pages.filter((p) => p.spaces && p.catalog).map((p) => p.slug);
  return pages.filter((p) => p.spaces && (p.catalog || !catalogs.some((c) => p.slug.startsWith(`${c}/`))));
}

// ----------------------------------------------------------------------------
// Index and navigation
// ----------------------------------------------------------------------------

/** One-line descriptions of the pages other generators own, for the index. */
const OTHER_PAGES: Record<string, [title: string, description: string]> = {
  'os-image-catalog': ['Image catalog details', 'The canonical images, their variants per backend and architecture.'],
  'image-software': ["What's installed", 'The apps, tools and simulator runtimes in each image tier, as the strict doctor measured them.'],
  'runtime-support': ['Runtime support', 'Which images run on which local runtime and in the cloud.'],
  python: ['Python high-level API', '`cua_sandbox`: `Sandbox`, `Image`, the computer interfaces, `Pool`, runtimes and configuration.'],
  typescript: ['TypeScript additions', 'What `@trycua/cua` adds in TypeScript: `embedded()`, `Image`, helpers, and the Spaces modules.'],
  rust: ['Rust crates', 'The public crates (`cua-sdk`, `cua-spacesd-client`, `cua-fleet`, `cua-sandbox-core`, `cua-spaces`), with stability tiers.'],
  protocol: ['Protocol', 'The gRPC contracts of cua-spacesd (`cua.env.v1`) and the cua daemon (`cua.daemon.v1`).'],
};

/**
 * The sidebar groups for `pages`: slugs of pages this set lacks are dropped,
 * and a top-level page no group lists fails, so nothing is orphaned.
 */
export function navGroups(pages: PageSpec[]): Array<[string, string[]]> {
  const own = new Set(pages.map((p) => p.slug));
  const known = (s: string) => own.has(s) || s === ERRORS_SLUG || s === TYPES_SLUG || (pages === PAGES && s in OTHER_PAGES);
  const listed = new Set(NAV.flatMap(([, slugs]) => slugs));
  const orphans = pages.filter((p) => !p.slug.includes('/') && !listed.has(p.slug));
  if (orphans.length) throw new Error(`pages missing from NAV in cua-sdk.ts: ${orphans.map((p) => p.slug).join(', ')}`);
  return NAV.map(([label, slugs]): [string, string[]] => [label, slugs.filter(known)]).filter(([, slugs]) => slugs.length);
}

function renderIndex(api: SdkApi, pages: PageSpec[], version: string, spaces?: SdkApi | null): string {
  const specs = new Map(pages.map((p) => [p.slug, p]));
  const body: string[] = [
    'The Cua SDK in Python, TypeScript, Swift, Kotlin and Rust, all generated from the Rust `cua-sdk` crate. Each page covers one object, with signatures in every language.',
    '',
  ];
  if (pages.some((p) => p.spaces)) {
    body.push(
      'Pages marked *Cua Spaces app export* document the APIs the Cua Spaces apps are built on, not the SDK: source-available under FSL-1.1-MIT, Swift only.',
      ''
    );
  }
  const row = (p: PageSpec) =>
    `| [${p.title}](${pageRoute(p.slug)}) | ${escapeTableCell(p.description)}${p.spaces ? ' *Cua Spaces app export.*' : ''} |`;
  const header = readHeader('cua-sdk/index.md', { version });
  if (header) body.push(header, '');
  for (const [label, slugs] of navGroups(pages)) {
    body.push(`## ${label}`, '', '| Page | Covers |', '| --- | --- |');
    for (const s of slugs) {
      const own = specs.get(s);
      if (own) {
        body.push(row(own));
        for (const sub of pages.filter((p) => p.slug.startsWith(`${s}/`))) body.push(row(sub));
      } else if (s === ERRORS_SLUG) {
        body.push(`| [Errors](${pageRoute(s)}) | Every \`CuaError\` variant: message, cause and fix. |`);
      } else if (s === TYPES_SLUG) {
        body.push(`| [Types](${pageRoute(s)}) | The type notation in each language, and every item in one list. |`);
      } else {
        const other = OTHER_PAGES[s];
        if (!other) throw new Error(`no index entry for ${s}`);
        body.push(`| [${other[0]}](${pageRoute(s)}) | ${escapeTableCell(other[1])} |`);
      }
    }
    body.push('');
  }
  const count = allItems(api).length;
  body.push(`${count} items, ${api.objects.reduce((n, o) => n + o.methods.length, 0)} methods. The [Types](${pageRoute(TYPES_SLUG)}) page lists them.`);
  if (spaces && pages.some((p) => p.spaces)) {
    body.push(
      '',
      `The Cua Spaces app export adds ${allItems(spaces, 'spaces').length} items and ${spaces.objects.reduce((n, o) => n + o.methods.length, 0)} methods.`
    );
  }
  return renderPage({
    title: 'Reference',
    description: 'The Cua SDK reference: every object with its signature in Python, TypeScript, Swift, Kotlin and Rust.',
    generator: GENERATOR,
    source: SOURCE,
    version,
    body: body.join('\n'),
  });
}

/** Folder `meta.json`s: the reference root (grouped) and each object folder. */
function navFiles(pages: PageSpec[], outputDir: string): Map<string, string> {
  const files = new Map<string, string>();
  // `index` is left out: each folder's index is the folder's own link, not a child.
  const root: string[] = [];
  for (const [label, slugs] of navGroups(pages)) root.push(`---${label}---`, ...slugs);
  files.set(path.join(outputDir, 'meta.json'), metaJson('Reference', root));
  for (const folder of pageFolders(pages)) {
    const home = pages.find((p) => p.slug === folder);
    if (!home) throw new Error(`pages under ${folder}/ need a \`${folder}\` page (the folder's index)`);
    // Direct children only: `spaces/app-core/notch` is listed by `spaces/app-core`.
    const children = pages
      .filter((p) => p.slug.startsWith(`${folder}/`) && !p.slug.slice(folder.length + 1).includes('/'))
      .map((p) => p.slug.slice(folder.length + 1));
    files.set(path.join(outputDir, folder, 'meta.json'), metaJson(home.title, children));
  }
  return files;
}

/** Every folder slug, at any depth (`spaces`, `spaces/app-core`), sorted. */
export function pageFolders(pages: PageSpec[]): string[] {
  const folders = new Set<string>();
  for (const p of pages) {
    const parts = p.slug.split('/');
    for (let i = 1; i < parts.length; i++) folders.add(parts.slice(0, i).join('/'));
  }
  return [...folders].sort();
}

/** The file a page slug is written to (`spacesd` -> `spacesd/index.mdx` when it is a folder). */
function pageFile(slugPath: string, pages: PageSpec[], outputDir: string): string {
  const isFolder = pages.some((p) => p.slug.startsWith(`${slugPath}/`));
  return path.join(outputDir, isFolder ? `${slugPath}/index.mdx` : `${slugPath}.mdx`);
}

export interface RenderOptions {
  renames?: Renames;
  outputDir?: string;
  pages?: PageSpec[];
  examples?: Map<string, Example[]>;
  /** `#[error]` templates of CuaError (read from lib.rs by default). */
  errorMessages?: Map<string, string>;
  /** The Cua Spaces app export (`cua_spaces_ffi`), for the `spaces` pages. */
  spaces?: SdkApi | null;
  /** The app export's `uniffi.toml` renames (cua-spaces-ffi by default). */
  spacesRenames?: Renames;
}

/** All generated files, keyed by absolute path. */
export function renderAll(api: SdkApi, version: string, opts: RenderOptions = {}): Map<string, string> {
  const renames = opts.renames ?? loadRenames();
  const outputDir = opts.outputDir ?? REFERENCE_DIR;
  const pages = opts.pages ?? PAGES;
  const examples = opts.examples ?? loadExamples(SDK_EXAMPLES);
  const messages = opts.errorMessages ?? errorMessages(fs.readFileSync(path.join(SDK_SRC, 'lib.rs'), 'utf-8'));
  const spaces = opts.spaces ?? null;
  const both = mergeApis(api, spaces);
  const typeContext = (renames: Renames): TypeContext => ({
    renames,
    traits: new Set(both.objects.filter((o) => o.trait_interface).map((o) => o.name)),
    errors: new Set(both.enums.filter((e) => e.error).map((e) => e.name)),
  });
  const ctx = typeContext(renames);
  const spacesCtx = typeContext(opts.spacesRenames ?? (spaces ? loadSpacesRenames() : {}));
  const placed = placeItems(api, pages, spaces);
  const links = linkMap(placed, both);
  const used = new Set<string>();
  const errorVariants = new Set(api.enums.filter((e) => e.error).flatMap((e) => e.variants.map((v) => v.name)));
  const all = [...allItems(api), ...(spaces ? allItems(spaces, 'spaces') : [])];
  const rcFor = (page: string, app = false): RenderContext =>
    app
      ? { ctx: spacesCtx, api: both, all, langs: SPACES_LANGS, links, examples, used, page, pages, errorVariants }
      : { ctx, api, all, langs: LANGS, links, examples, used, page, pages, errorVariants };
  const files = new Map<string, string>();
  for (const [slugPath, p] of placed) {
    files.set(pageFile(slugPath, pages, outputDir), renderObjectPage(p, rcFor(slugPath, p.spec.spaces), version));
  }
  files.set(path.join(outputDir, `${ERRORS_SLUG}.mdx`), renderErrorsPage(api, rcFor(ERRORS_SLUG), version, messages));
  files.set(path.join(outputDir, `${TYPES_SLUG}.mdx`), renderTypesPage(api, rcFor(TYPES_SLUG), version, renames));
  files.set(path.join(outputDir, 'index.mdx'), renderIndex(api, pages, version, spaces));
  for (const [file, content] of navFiles(pages, outputDir)) files.set(file, content);
  const unused = [...examples.keys()].filter((k) => !used.has(k));
  if (unused.length) {
    throw new Error(`examples with no matching item or method: ${unused.join(', ')} (scripts/docs-generators/examples/cua-sdk)`);
  }
  return files;
}

/** Problems a rendered page set has: duplicate anchors, oversized pages. */
export function pageProblems(files: Map<string, string>, maxWords = MAX_WORDS): string[] {
  const problems: string[] = [];
  for (const [file, content] of files) {
    if (!file.endsWith('.mdx')) continue;
    const seen = new Set<string>();
    let fence = false;
    for (const line of content.split('\n')) {
      if (/^\s*```/.test(line)) fence = !fence;
      const m = !fence && line.match(/^#{2,6}\s+(.*)$/);
      if (!m) continue;
      const anchor = slug(m[1].replace(/`/g, ''));
      if (seen.has(anchor)) problems.push(`${path.basename(file)}: duplicate anchor #${anchor}`);
      seen.add(anchor);
    }
    const words = wordCount(content);
    if (words > maxWords) problems.push(`${path.relative(REFERENCE_DIR, file)}: ${words} words (max ${maxWords}); split it in PAGES`);
    const fences = (content.match(/^\s*```/gm) ?? []).length / 2;
    const tabCount = (content.match(/<Tabs\b/g) ?? []).length;
    if (fences > MAX_FENCES || tabCount > MAX_TABS) {
      problems.push(`${path.relative(REFERENCE_DIR, file)}: ${fences} code blocks, ${tabCount} Tabs (max ${MAX_FENCES}, ${MAX_TABS}); split it in PAGES`);
    }
    const bytes = Buffer.byteLength(content);
    if (bytes > MAX_BYTES) problems.push(`${path.relative(REFERENCE_DIR, file)}: ${bytes} bytes (max ${MAX_BYTES}); split it in PAGES`);
  }
  return problems;
}

// ============================================================================
// Binding cross-check
// ============================================================================

export interface BindingSources {
  /** A binding left out is not checked (the app export has a Swift binding only). */
  python?: string;
  typescript?: string;
  swift: string;
  kotlin?: string;
  /** cua-sdk's `src/lib.rs`, `src/types.rs` and `src/native/*.rs`, concatenated. */
  rust?: string;
}

export function loadBindingSources(root = CUA_ROOT): BindingSources {
  const read = (p: string) => fs.readFileSync(path.join(root, p), 'utf-8');
  const native = path.join(root, 'crates', 'cua-sdk', 'src', 'native');
  const rust = [
    read('crates/cua-sdk/src/lib.rs'),
    read('crates/cua-sdk/src/types.rs'),
    ...fs
      .readdirSync(native)
      .filter((f) => f.endsWith('.rs'))
      .sort()
      .map((f) => fs.readFileSync(path.join(native, f), 'utf-8')),
  ].join('\n');
  return {
    python: read('python/src/cua/_native.py'),
    typescript: read('typescript/src/native/cua_sdk.ts'),
    swift: read('swift/Sources/CuaSDK/CuaSDK.swift'),
    kotlin: read('kotlin/src/main/kotlin/ai/cua/sdk/cua_sdk.kt'),
    rust,
  };
}

export function loadRenames(root = CUA_ROOT): Renames {
  return parseRenames(fs.readFileSync(path.join(root, 'crates', 'cua-sdk', 'uniffi.toml'), 'utf-8'));
}

/** The app export's names: its Swift binding and the cua-spaces-ffi source (no Python, TypeScript or Kotlin binding exists). */
export function loadSpacesBindingSources(crate = SPACES_FFI_ROOT, swift = SPACES_SWIFT): BindingSources {
  const src = path.join(crate, 'src');
  const rust = fs
    .readdirSync(src)
    .filter((f) => f.endsWith('.rs'))
    .sort()
    .map((f) => fs.readFileSync(path.join(src, f), 'utf-8'))
    .join('\n');
  return { swift: fs.readFileSync(swift, 'utf-8'), rust };
}

export function loadSpacesRenames(crate = SPACES_FFI_ROOT): Renames {
  return parseRenames(fs.readFileSync(path.join(crate, 'uniffi.toml'), 'utf-8'));
}

/**
 * Every name the pages print must appear in the committed bindings (and the
 * Rust source) in the form the naming rules predict. Returns the mismatches.
 */
export function verifyBindingNames(api: SdkApi, src: BindingSources, renames: Renames = {}): string[] {
  const problems: string[] = [];
  const expect = (lang: string, text: string | undefined, needle: string | RegExp, what: string) => {
    if (text === undefined) return;
    const ok = typeof needle === 'string' ? text.includes(needle) : needle.test(text);
    if (!ok) problems.push(`${lang}: ${what} (expected ${needle})`);
  };
  const esc = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  // UniFFI backtick-escapes Swift keywords used as identifiers (`open`, `switch`).
  const SWIFT_KEYWORDS = new Set(['open', 'switch', 'default', 'case', 'class', 'func', 'import', 'in', 'is', 'let', 'return', 'self', 'static', 'struct', 'super', 'var', 'where', 'while', 'protocol', 'public', 'private', 'internal', 'extension', 'enum', 'repeat', 'guard', 'defer', 'do', 'else', 'if', 'for', 'break', 'continue', 'fallthrough', 'throw', 'throws', 'try', 'catch', 'operator', 'init', 'deinit', 'subscript', 'typealias', 'associatedtype', 'inout', 'as', 'true', 'false', 'nil', 'Any', 'Type']);
  const swiftId = (n: string) => (SWIFT_KEYWORDS.has(n) ? `\`${n}\`` : n);
  // A word boundary can't follow a closing backtick, so escaped names end on the backtick itself.
  const swiftIdRe = (n: string) => (SWIFT_KEYWORDS.has(n) ? esc(swiftId(n)) : `${esc(n)}\\b`);
  const rustFn = (c: SdkCallable, what: string, opts: { primary?: boolean; trait?: boolean } = {}) => {
    if (src.rust === undefined) return;
    const name = opts.primary ? 'new' : c.name;
    // Trait methods are declared (`fn x(&self)`), not defined with `pub`.
    const re = opts.trait
      ? new RegExp(`${c.async ? 'async ' : ''}fn ${esc(name)}\\b`)
      : new RegExp(`pub ${c.async ? 'async ' : ''}fn ${esc(name)}\\b`);
    expect('Rust', src.rust, re, what);
  };
  const callable = (owner: string | null, c: SdkCallable, trait = false) => {
    const what = `${owner ?? '(function)'}.${c.name}`;
    const n = (lang: Lang) => esc(renamed(lang, owner, c.name, renames));
    expect('Python', src.python, new RegExp(`def ${n('Python')}\\(`), what);
    expect('TypeScript', src.typescript, new RegExp(`\\b${n('TypeScript')}\\(`), what);
    expect('Swift', src.swift, new RegExp(`func ${n('Swift')}\\(`), what);
    expect('Kotlin', src.kotlin, `fun \`${renamed('Kotlin', owner, c.name, renames)}\`(`, what);
    rustFn(c, what, { trait });
  };
  for (const o of api.objects) {
    expect('Python', src.python, `class ${o.name}`, o.name);
    expect('TypeScript', src.typescript, o.trait_interface ? `export interface ${o.name} ` : `export class ${o.name} `, o.name);
    expect('Swift', src.swift, o.trait_interface ? `public protocol ${o.name}:` : `open class ${o.name}:`, o.name);
    expect('Kotlin', src.kotlin, new RegExp(`(class|interface) ${esc(o.name)}\\b`), o.name);
    if (src.rust !== undefined) {
      expect('Rust', src.rust, new RegExp(`pub (struct|trait) ${esc(o.name)}\\b`), o.name);
    }
    for (const m of o.methods) callable(o.name, m, o.trait_interface);
    for (const c of o.constructors) {
      if (c.primary) {
        expect('TypeScript', src.typescript, new RegExp(`constructor\\(${c.arguments.map((a) => esc(lowerCamel(a.name)) + ':').join('.*')}`), `${o.name} constructor`);
        rustFn(c, `${o.name} constructor`, { primary: true });
      } else {
        callable(o.name, c);
      }
    }
  }
  for (const f of api.functions) callable(null, f);
  for (const r of api.records) {
    expect('Python', src.python, `class ${r.name}:`, r.name);
    expect('TypeScript', src.typescript, `export type ${r.name} = {`, r.name);
    expect('Swift', src.swift, `public struct ${r.name}`, r.name);
    expect('Kotlin', src.kotlin, `data class ${r.name} (`, r.name);
    if (src.rust !== undefined) expect('Rust', src.rust, new RegExp(`pub struct ${esc(r.name)}\\b`), r.name);
    for (const f of r.fields) {
      expect('Python', src.python, new RegExp(`\\b${esc(memberName('Python', f.name))}:`), `${r.name}.${f.name}`);
      expect('TypeScript', src.typescript, new RegExp(`\\b${esc(lowerCamel(f.name))}\\??:`), `${r.name}.${f.name}`);
      expect('Swift', src.swift, `public var ${swiftId(lowerCamel(f.name))}:`, `${r.name}.${f.name}`);
      expect('Kotlin', src.kotlin, `var \`${lowerCamel(f.name)}\`:`, `${r.name}.${f.name}`);
      if (src.rust !== undefined) expect('Rust', src.rust, new RegExp(`pub ${esc(f.name)}:`), `${r.name}.${f.name}`);
    }
  }
  for (const e of api.enums) {
    const kotlinName = e.error ? errorName('Kotlin', e.name) : e.name;
    expect('Python', src.python, new RegExp(`class ${esc(e.name)}\\b`), e.name);
    expect('Swift', src.swift, `public enum ${e.name}`, e.name);
    expect('Kotlin', src.kotlin, new RegExp(`class ${esc(kotlinName)}\\b`), e.name);
    expect('TypeScript', src.typescript, new RegExp(`export (enum|const) ${esc(e.name)}\\b`), e.name);
    if (src.rust !== undefined) expect('Rust', src.rust, new RegExp(`pub enum ${esc(e.name)}\\b`), e.name);
    for (const v of e.variants) {
      const what = `${e.name}.${v.name}`;
      const py = variantName('Python', e, v.name);
      const sw = variantName('Swift', e, v.name);
      const kt = variantName('Kotlin', e, v.name);
      if (e.error) {
        expect('Python', src.python, `class ${py}(`, what);
        expect('Swift', src.swift, `case ${swiftId(sw)}(`, what);
        expect('Kotlin', src.kotlin, new RegExp(`class ${esc(kt)}\\(`), what);
        expect('TypeScript', src.typescript, `class ${v.name} extends UniffiError`, what);
      } else if (e.flat) {
        expect('Python', src.python, new RegExp(`^    ${esc(py)} = `, 'm'), what);
        expect('Swift', src.swift, new RegExp(`^    case ${swiftIdRe(sw)}`, 'm'), what);
        expect('Kotlin', src.kotlin, new RegExp(`^    ${esc(kt)}\\b`, 'm'), what);
        expect('TypeScript', src.typescript, new RegExp(`^    ${esc(v.name)}\\b`, 'm'), what);
      } else {
        expect('Python', src.python, new RegExp(`class ${esc(py)}:`), what);
        expect('Swift', src.swift, new RegExp(`case ${swiftIdRe(sw)}`), what);
        expect('Kotlin', src.kotlin, new RegExp(`(class|object) ${esc(kt)}\\b`), what);
        expect('TypeScript', src.typescript, `${v.name} = "${v.name}"`, what);
      }
      if (src.rust !== undefined) expect('Rust', src.rust, new RegExp(`^\\s*${esc(v.name)}\\b`, 'm'), what);
    }
  }
  return problems;
}

// ============================================================================
// Build and dump
// ============================================================================

function cargo(): string {
  return process.env.CARGO || 'cargo';
}

function libraryFile(name = 'cua_sdk'): string {
  if (process.platform === 'darwin') return `lib${name}.dylib`;
  if (process.platform === 'win32') return `${name}.dll`;
  return `lib${name}.so`;
}

/**
 * The SDK dump (`cua_sdk` from libcua_sdk) and the Cua Spaces app export
 * dump (`cua_spaces_ffi` from libcua_spaces_ffi), building what the
 * environment does not provide.
 */
export function loadApis(): { api: SdkApi; spaces: SdkApi } {
  const fromJson = (file: string | undefined) => (file ? (JSON.parse(fs.readFileSync(file, 'utf-8')) as SdkApi) : null);
  let api = fromJson(process.env.CUA_SDK_API_JSON);
  let spaces = fromJson(process.env.CUA_SPACES_FFI_API_JSON);
  if (api && spaces) return { api, spaces };
  const target = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(CUA_ROOT, 'target');
  const packages = [
    ...(api || process.env.CUA_SDK_LIBRARY ? [] : ['-p', 'cua-sdk']),
    ...(spaces || process.env.CUA_SPACES_FFI_LIBRARY ? [] : ['-p', 'cua-spaces-ffi']),
    ...(process.env.CUA_BINDGEN ? [] : ['-p', 'cua-bindgen']),
  ];
  if (packages.length) {
    console.log(`Building ${packages.filter((p) => p !== '-p').join(', ')} (debug; UniFFI metadata is profile-independent)...`);
    execFileSync(cargo(), ['build', '--locked', ...packages], { cwd: CUA_ROOT, stdio: 'inherit' });
  }
  const bindgen =
    process.env.CUA_BINDGEN || path.join(target, 'debug', process.platform === 'win32' ? 'cua-bindgen.exe' : 'cua-bindgen');
  const dump = (library: string, namespace: string): SdkApi =>
    JSON.parse(
      execFileSync(bindgen, ['docs', '--library', library, '--namespace', namespace], {
        cwd: CUA_ROOT,
        encoding: 'utf-8',
        maxBuffer: 64 * 1024 * 1024,
      })
    );
  api ??= dump(process.env.CUA_SDK_LIBRARY || path.join(target, 'debug', libraryFile('cua_sdk')), 'cua_sdk');
  spaces ??= dump(process.env.CUA_SPACES_FFI_LIBRARY || path.join(target, 'debug', libraryFile('cua_spaces_ffi')), 'cua_spaces_ffi');
  return { api, spaces };
}

export function loadApi(): SdkApi {
  return loadApis().api;
}

/** Directories whose generated pages this generator owns (stale ones are removed). */
function ownedDirs(pages: PageSpec[] = PAGES): string[] {
  return [REFERENCE_DIR, ...pageFolders(pages).map((f) => path.join(REFERENCE_DIR, f))];
}

function main(): void {
  const checkOnly = isCheckMode();
  console.log('Cua SDK reference (UniFFI)');
  const { api, spaces } = loadApis();
  const renames = loadRenames();
  const spacesRenames = loadSpacesRenames();
  const problems = verifyBindingNames(api, loadBindingSources(), renames);
  if (problems.length) {
    console.error(`The rendered names disagree with the committed bindings or the Rust source (${problems.length}):`);
    for (const p of problems.slice(0, 40)) console.error(`  ${p}`);
    console.error('Regenerate the bindings (libs/cua/scripts/generate-uniffi-bindings.mjs) or fix the naming rules in cua-sdk.ts.');
    process.exit(1);
  }
  const spacesProblems = verifyBindingNames(spaces, loadSpacesBindingSources(), spacesRenames);
  if (spacesProblems.length) {
    console.error(`The Cua Spaces app export's names disagree with its Swift binding or the cua-spaces-ffi source (${spacesProblems.length}):`);
    for (const p of spacesProblems.slice(0, 40)) console.error(`  ${p}`);
    console.error('Regenerate the Swift binding (node libs/spaces-app-swift/scripts/generate-bindings.mjs) or fix the naming rules in cua-sdk.ts.');
    process.exit(1);
  }
  const version = fs.readFileSync(path.join(CUA_ROOT, 'VERSION'), 'utf-8').trim();
  const files = renderAll(api, version, { renames, spaces, spacesRenames });
  const pageIssues = pageProblems(files);
  if (pageIssues.length) {
    for (const p of pageIssues) console.error(`  ${p}`);
    process.exit(1);
  }
  const drift = syncFiles(files, checkOnly, ownedDirs(), GENERATOR);
  finish('Cua SDK', drift, checkOnly, GENERATOR);
}

if (require.main === module) {
  main();
}
