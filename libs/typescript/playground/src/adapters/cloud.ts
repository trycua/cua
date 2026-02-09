// Cloud adapter implementation
// Uses CUA API for persistence, VMs, and inference configuration

import type {
  PlaygroundAdapters,
  PersistenceAdapter,
  ComputerAdapter,
  InferenceAdapter,
  CloudAdapterConfig,
  ComputerInfo,
  InferenceConfig,
} from './types';
import type { AgentMessage, Chat, ModelProvider } from '../types';

// =============================================================================
// API Error Helper
// =============================================================================

class ApiError extends Error {
  constructor(
    public status: number,
    message: string
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

// =============================================================================
// Cloud Persistence Adapter
// =============================================================================

class CloudPersistenceAdapter implements PersistenceAdapter {
  constructor(
    private apiKey: string,
    private baseUrl: string
  ) {}

  private async fetch<T>(path: string, options?: RequestInit): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      ...options,
      headers: {
        Authorization: `Bearer ${this.apiKey}`,
        'Content-Type': 'application/json',
        ...options?.headers,
      },
    });

    if (!response.ok) {
      let errorMessage: string;
      try {
        const errorBody = await response.json();
        errorMessage = errorBody.detail || errorBody.error || `HTTP ${response.status}`;
      } catch {
        errorMessage = `HTTP ${response.status}`;
      }
      throw new ApiError(response.status, errorMessage);
    }

    return response.json();
  }

  async loadChats(): Promise<Chat[]> {
    return this.fetch<Chat[]>('/v1/playground/chats');
  }

  async saveChat(chat: Chat): Promise<Chat> {
    if (chat.id) {
      return this.fetch<Chat>(`/v1/playground/chats/${chat.id}`, {
        method: 'PUT',
        body: JSON.stringify(chat),
      });
    }
    return this.fetch<Chat>('/v1/playground/chats', {
      method: 'POST',
      body: JSON.stringify(chat),
    });
  }

  async deleteChat(chatId: string): Promise<void> {
    await this.fetch(`/v1/playground/chats/${chatId}`, { method: 'DELETE' });
  }

  async saveMessages(chatId: string, messages: AgentMessage[]): Promise<void> {
    await this.fetch(`/v1/playground/chats/${chatId}/messages`, {
      method: 'PUT',
      body: JSON.stringify({ messages }),
    });
  }

  async loadSettings<T>(key: string): Promise<T | null> {
    try {
      return await this.fetch<T>(`/v1/playground/settings/${key}`);
    } catch (error) {
      // Return null if settings not found (404)
      if (error instanceof ApiError && error.status === 404) {
        return null;
      }
      throw error;
    }
  }

  async saveSettings<T>(key: string, value: T): Promise<void> {
    await this.fetch(`/v1/playground/settings/${key}`, {
      method: 'PUT',
      body: JSON.stringify({ value }),
    });
  }
}

// =============================================================================
// Cloud Computer Adapter
// =============================================================================

// VM response type from API
interface VMResponse {
  id: string;
  vmId?: string;
  name: string;
  customName?: string;
  status: string;
  vncUrl?: string | null;
  vnc_url?: string | null;
  agentUrl?: string | null;
  agent_url?: string | null;
}

class CloudComputerAdapter implements ComputerAdapter {
  constructor(
    private apiKey: string,
    private baseUrl: string
  ) {}

  private async fetch<T>(path: string, options?: RequestInit): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      ...options,
      headers: {
        Authorization: `Bearer ${this.apiKey}`,
        'Content-Type': 'application/json',
        ...options?.headers,
      },
    });

    if (!response.ok) {
      let errorMessage: string;
      try {
        const errorBody = await response.json();
        errorMessage = errorBody.detail || errorBody.error || `HTTP ${response.status}`;
      } catch {
        errorMessage = `HTTP ${response.status}`;
      }
      throw new ApiError(response.status, errorMessage);
    }

    return response.json();
  }

  async listComputers(): Promise<ComputerInfo[]> {
    const vms = await this.fetch<VMResponse[]>('/v1/vms');
    return vms.map((vm) => ({
      id: vm.vmId || vm.id,
      name: vm.customName || vm.name || `VM ${vm.id}`,
      vncUrl: vm.vncUrl || vm.vnc_url || '',
      agentUrl: vm.agentUrl || vm.agent_url || '',
      status: this.mapVMStatus(vm.status),
    }));
  }

  private mapVMStatus(status: string): 'running' | 'stopped' | 'starting' | 'error' {
    switch (status.toLowerCase()) {
      case 'running':
        return 'running';
      case 'stopped':
      case 'suspended':
        return 'stopped';
      case 'starting':
      case 'provisioning':
        return 'starting';
      default:
        return 'error';
    }
  }

  async getDefaultComputer(): Promise<ComputerInfo | null> {
    const computers = await this.listComputers();
    // Prefer a running computer, otherwise return the first one
    return computers.find((c) => c.status === 'running') ?? computers[0] ?? null;
  }

  async checkHealth(computerId: string): Promise<boolean> {
    try {
      await this.fetch(`/v1/vms/${computerId}/health`);
      return true;
    } catch {
      return false;
    }
  }
}

// =============================================================================
// Cloud Inference Adapter
// =============================================================================

class CloudInferenceAdapter implements InferenceAdapter {
  constructor(
    private apiKey: string,
    private baseUrl: string
  ) {}

  async getConfig(computer: ComputerInfo): Promise<InferenceConfig> {
    // Cloud manages API keys server-side, so we don't need to pass env vars
    return {
      baseUrl: computer.agentUrl,
    };
  }

  async getAvailableModels(): Promise<ModelProvider[]> {
    try {
      const response = await fetch(`${this.baseUrl}/v1/models`, {
        headers: { Authorization: `Bearer ${this.apiKey}` },
      });

      if (!response.ok) {
        // Return default models if endpoint not available
        return this.getDefaultModels();
      }

      return response.json();
    } catch {
      // Return default models on error
      return this.getDefaultModels();
    }
  }

  private getDefaultModels(): ModelProvider[] {
    return [
      {
        name: 'Anthropic',
        models: [
          { id: 'claude-sonnet-4-20250514', name: 'Claude Sonnet 4' },
          { id: 'claude-3-5-sonnet-20241022', name: 'Claude 3.5 Sonnet' },
        ],
      },
    ];
  }
}

// =============================================================================
// Factory Function
// =============================================================================

/**
 * Create a cloud adapter bundle for the playground.
 *
 * @param config - Configuration for the cloud adapter
 * @returns PlaygroundAdapters bundle for cloud usage
 *
 * @example
 * ```typescript
 * const adapters = createCloudAdapter({
 *   apiKey: userApiKey,
 *   baseUrl: 'https://api.cua.ai', // optional
 * });
 *
 * <Playground adapters={adapters} />
 * ```
 */
export function createCloudAdapter(config: CloudAdapterConfig): PlaygroundAdapters {
  const baseUrl = config.baseUrl || 'https://api.cua.ai';

  return {
    persistence: new CloudPersistenceAdapter(config.apiKey, baseUrl),
    computer: new CloudComputerAdapter(config.apiKey, baseUrl),
    inference: new CloudInferenceAdapter(config.apiKey, baseUrl),
  };
}
