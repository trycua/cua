// Local adapter implementation
// Uses localStorage for persistence, user-provided computer URLs, and user-provided API keys

import type {
  PlaygroundAdapters,
  PersistenceAdapter,
  ComputerAdapter,
  InferenceAdapter,
  LocalAdapterConfig,
  ComputerInfo,
  InferenceConfig,
} from './types';
import type { AgentMessage, Chat, ModelProvider } from '../types';
import {
  loadItemsFromLocalStorage,
  saveItemsToLocalStorage,
  loadItemFromLocalStorage,
  saveItemToLocalStorage,
} from '../utils/localStorage';

// =============================================================================
// Storage Keys
// =============================================================================

const STORAGE_KEYS = {
  CHATS: 'playground-chats',
  CUSTOM_COMPUTERS: 'playground-custom-computers',
  SETTINGS: 'playground-settings',
} as const;

// =============================================================================
// Local Persistence Adapter
// =============================================================================

class LocalPersistenceAdapter implements PersistenceAdapter {
  async loadChats(): Promise<Chat[]> {
    return loadItemsFromLocalStorage<Chat>({ storageKey: STORAGE_KEYS.CHATS });
  }

  async saveChat(chat: Chat): Promise<Chat> {
    const chats = await this.loadChats();
    const index = chats.findIndex((c) => c.id === chat.id);
    if (index >= 0) {
      chats[index] = chat;
    } else {
      chats.push(chat);
    }
    saveItemsToLocalStorage(chats, { storageKey: STORAGE_KEYS.CHATS });
    return chat;
  }

  async deleteChat(chatId: string): Promise<void> {
    const chats = await this.loadChats();
    saveItemsToLocalStorage(
      chats.filter((c) => c.id !== chatId),
      { storageKey: STORAGE_KEYS.CHATS }
    );
  }

  async saveMessages(chatId: string, messages: AgentMessage[]): Promise<void> {
    const chats = await this.loadChats();
    const chat = chats.find((c) => c.id === chatId);
    if (chat) {
      chat.messages = messages;
      await this.saveChat(chat);
    }
  }

  async loadSettings<T>(key: string): Promise<T | null> {
    const settings = loadItemFromLocalStorage<Record<string, unknown>>({
      storageKey: STORAGE_KEYS.SETTINGS,
    });
    if (!settings) return null;
    return (settings[key] as T) ?? null;
  }

  async saveSettings<T>(key: string, value: T): Promise<void> {
    const settings =
      loadItemFromLocalStorage<Record<string, unknown>>({
        storageKey: STORAGE_KEYS.SETTINGS,
      }) || {};
    settings[key] = value;
    saveItemToLocalStorage(settings, { storageKey: STORAGE_KEYS.SETTINGS });
  }
}

// =============================================================================
// Local Computer Adapter
// =============================================================================

class LocalComputerAdapter implements ComputerAdapter {
  constructor(private config: LocalAdapterConfig) {}

  async listComputers(): Promise<ComputerInfo[]> {
    const customComputers = loadItemsFromLocalStorage<ComputerInfo>({
      storageKey: STORAGE_KEYS.CUSTOM_COMPUTERS,
    });

    // If a default computer URL is configured, add it
    if (this.config.computerServerUrl) {
      const defaultComputer: ComputerInfo = {
        id: 'default',
        name: 'Local Computer',
        vncUrl: `${this.config.computerServerUrl}/vnc`,
        agentUrl: this.config.computerServerUrl,
        status: 'running',
      };
      return [defaultComputer, ...customComputers];
    }

    return customComputers;
  }

  async getDefaultComputer(): Promise<ComputerInfo | null> {
    const computers = await this.listComputers();
    return computers[0] ?? null;
  }

  async addCustomComputer(
    computer: Omit<ComputerInfo, 'id'>
  ): Promise<ComputerInfo> {
    const newComputer: ComputerInfo = {
      ...computer,
      id: crypto.randomUUID(),
      isCustom: true,
    };
    const computers = loadItemsFromLocalStorage<ComputerInfo>({
      storageKey: STORAGE_KEYS.CUSTOM_COMPUTERS,
    });
    computers.push(newComputer);
    saveItemsToLocalStorage(computers, {
      storageKey: STORAGE_KEYS.CUSTOM_COMPUTERS,
    });
    return newComputer;
  }

  async removeCustomComputer(computerId: string): Promise<void> {
    const computers = loadItemsFromLocalStorage<ComputerInfo>({
      storageKey: STORAGE_KEYS.CUSTOM_COMPUTERS,
    });
    saveItemsToLocalStorage(
      computers.filter((c) => c.id !== computerId),
      { storageKey: STORAGE_KEYS.CUSTOM_COMPUTERS }
    );
  }

  async checkHealth(computerId: string): Promise<boolean> {
    const computers = await this.listComputers();
    const computer = computers.find((c) => c.id === computerId);
    if (!computer) return false;

    try {
      const response = await fetch(`${computer.agentUrl}/health`, {
        method: 'GET',
        signal: AbortSignal.timeout(5000),
      });
      return response.ok;
    } catch {
      return false;
    }
  }
}

// =============================================================================
// Local Inference Adapter
// =============================================================================

class LocalInferenceAdapter implements InferenceAdapter {
  constructor(private config: LocalAdapterConfig) {}

  async getConfig(computer: ComputerInfo): Promise<InferenceConfig> {
    const env: Record<string, string> = {};

    if (this.config.providerApiKeys?.anthropic) {
      env.ANTHROPIC_API_KEY = this.config.providerApiKeys.anthropic;
    }
    if (this.config.providerApiKeys?.openai) {
      env.OPENAI_API_KEY = this.config.providerApiKeys.openai;
    }

    return {
      baseUrl: computer.agentUrl,
      env,
    };
  }

  async getAvailableModels(): Promise<ModelProvider[]> {
    const providers: ModelProvider[] = [];

    if (this.config.providerApiKeys?.anthropic) {
      providers.push({
        name: 'Anthropic',
        models: [
          { id: 'claude-sonnet-4-20250514', name: 'Claude Sonnet 4' },
          { id: 'claude-3-5-sonnet-20241022', name: 'Claude 3.5 Sonnet' },
        ],
      });
    }

    if (this.config.providerApiKeys?.openai) {
      providers.push({
        name: 'OpenAI',
        models: [
          { id: 'gpt-4o', name: 'GPT-4o' },
          { id: 'gpt-4o-mini', name: 'GPT-4o Mini' },
        ],
      });
    }

    return providers;
  }
}

// =============================================================================
// Factory Function
// =============================================================================

/**
 * Create a local adapter bundle for the playground.
 *
 * @param config - Configuration for the local adapter
 * @returns PlaygroundAdapters bundle for local usage
 *
 * @example
 * ```typescript
 * const adapters = createLocalAdapter({
 *   computerServerUrl: 'http://localhost:8443',
 *   providerApiKeys: {
 *     anthropic: process.env.ANTHROPIC_API_KEY,
 *   },
 * });
 *
 * <Playground adapters={adapters} />
 * ```
 */
export function createLocalAdapter(
  config: LocalAdapterConfig = {}
): PlaygroundAdapters {
  return {
    persistence: new LocalPersistenceAdapter(),
    computer: new LocalComputerAdapter(config),
    inference: new LocalInferenceAdapter(config),
  };
}
