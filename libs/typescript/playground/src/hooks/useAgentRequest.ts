// Hook for making agent requests
// Adapted from cloud/src/website/app/hooks/playground/useAgentRequest.ts

import { useRef, useCallback } from 'react';
import { usePlayground, useChat, useChatDispatch } from './usePlayground';
import { usePlaygroundTelemetry } from '../telemetry';
import type { AgentMessage, UserMessage } from '../types';
import { isVM, isCustomComputer } from '../types';

// Agent client interface for making requests
interface AgentClientOptions {
  timeout?: number;
  retries?: number;
  signal?: AbortSignal;
  apiKey?: string;
  onRetry?: (attempt: number, maxRetries: number) => void;
}

interface AgentResponse {
  status?: string;
  error?: string;
  output?: AgentMessage[];
}

/**
 * Simple agent client for making requests to the computer server.
 */
class AgentClient {
  constructor(
    private baseUrl: string,
    private options: AgentClientOptions & { env?: Record<string, string> } = {}
  ) {}

  async health(): Promise<{ status: 'ok' | 'unreachable' }> {
    try {
      // Try /cmd endpoint (cloud sandboxes use this for health checks)
      await fetch(`${this.baseUrl}/cmd`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ command: 'version', params: {} }),
        signal: this.options.signal || AbortSignal.timeout(5000),
      });
      // Consider server reachable if we get any response (even 4xx means server is up)
      return { status: 'ok' };
    } catch {
      return { status: 'unreachable' };
    }
  }

  responses = {
    create: async (params: {
      model: string;
      input: (UserMessage | AgentMessage)[];
      agent_kwargs?: Record<string, unknown>;
      env?: Record<string, string>;
    }): Promise<AgentResponse> => {
      const { timeout = 120000, retries = 3, signal, onRetry } = this.options;

      let lastError: Error | null = null;

      for (let attempt = 0; attempt <= retries; attempt++) {
        try {
          if (attempt > 0) {
            onRetry?.(attempt, retries);
          }

          const headers: Record<string, string> = {
            'Content-Type': 'application/json',
          };
          if (this.options.apiKey) {
            headers['X-API-Key'] = this.options.apiKey;
          }

          const response = await fetch(`${this.baseUrl}/responses`, {
            method: 'POST',
            headers,
            body: JSON.stringify({
              model: params.model,
              input: params.input,
              agent_kwargs: params.agent_kwargs,
              env: { ...this.options.env, ...params.env },
            }),
            signal: signal || AbortSignal.timeout(timeout),
          });

          if (!response.ok) {
            throw new Error(`Request failed: ${response.status}`);
          }

          return await response.json();
        } catch (error) {
          lastError = error instanceof Error ? error : new Error(String(error));
          if (signal?.aborted) throw lastError;
        }
      }

      throw lastError || new Error('Request failed');
    },
  };
}

/**
 * Hook for making agent requests.
 * Handles the agent loop, abort handling, and retry logic.
 *
 * @returns Object with sendMessage, stopResponse, and retry handlers
 */
export function useAgentRequest() {
  const { adapters, state, dispatch: playgroundDispatch } = usePlayground();
  const chatState = useChat();
  const chatDispatch = useChatDispatch();
  const {
    trackMessageSent,
    trackTrajectoryCompleted,
    trackTrajectoryFailed,
    trackTrajectoryStopped,
  } = usePlaygroundTelemetry();

  const shouldStopResponseRef = useRef(false);
  const abortReasonRef = useRef<'manual' | 'timeout' | null>(null);
  const currentRequestRef = useRef<{
    chatId: string;
    messages: (UserMessage | AgentMessage)[];
    abortController?: AbortController;
    startTime?: number;
    iterationCount?: number;
  } | null>(null);

  // Keep a ref to the latest chat state to avoid stale closure issues
  // Update synchronously during render (not in useEffect) to ensure it's always current
  const chatStateRef = useRef(chatState);
  chatStateRef.current = chatState;

  // Helper to set generating state globally
  const setGlobalGenerating = useCallback(
    (chatId: string, generating: boolean) => {
      playgroundDispatch({
        type: 'SET_CHAT_GENERATING',
        payload: { chatId, generating },
      });
    },
    [playgroundDispatch]
  );

  // Helper to update messages in BOTH local and global state
  const setMessages = useCallback(
    (chatId: string, messages: (UserMessage | AgentMessage)[]) => {
      chatDispatch({ type: 'SET_MESSAGES', payload: messages as AgentMessage[] });
      playgroundDispatch({
        type: 'SET_CHAT_MESSAGES',
        payload: { id: chatId, messages: messages as AgentMessage[] },
      });
    },
    [chatDispatch, playgroundDispatch]
  );

  // Persist messages via adapter
  const persistMessages = useCallback(
    async (chatId: string, messages: (UserMessage | AgentMessage)[]) => {
      try {
        await adapters.persistence.saveMessages(chatId, messages as AgentMessage[]);
      } catch (error) {
        console.error('Failed to persist messages:', error);
      }
    },
    [adapters.persistence]
  );

  // Persist title via adapter
  const persistTitle = useCallback(
    async (chatId: string, title: string) => {
      try {
        const chat = state.chats.find((c) => c.id === chatId);
        if (chat) {
          await adapters.persistence.saveChat({ ...chat, name: title });
        }
      } catch (error) {
        console.error('Failed to persist title:', error);
      }
    },
    [adapters.persistence, state.chats]
  );

  const sendAgentRequest = useCallback(
    async (messages: (UserMessage | AgentMessage)[], abortController: AbortController) => {
      // Use ref to get the latest chat state to avoid stale closure issues
      const latestChatState = chatStateRef.current;
      const { model, computer } = latestChatState;

      if (!model || !computer) {
        return;
      }

      // Reset request start time for each iteration
      const chatId = currentRequestRef.current?.chatId;
      if (chatId) {
        playgroundDispatch({
          type: 'RESET_REQUEST_START_TIME',
          payload: { chatId },
        });
      }

      // Clear retry state
      chatDispatch({ type: 'SET_RETRY_STATE', payload: null });

      try {
        // Get computer server URL - try multiple sources in order of preference:
        // 1. ComputerInfo.agentUrl from state.computers (set by adapter with correct port)
        // 2. computer.url from the chat's Computer object (set to agentUrl when selected)
        // 3. Fallback: reconstruct from hostname with port 8443 (legacy cloud VMs)
        const computerInfo = state.computers.find((c) => c.id === computer.id);
        let computerServerUrl = computerInfo?.agentUrl || '';

        if (!computerServerUrl && isCustomComputer(computer) && computer.url) {
          computerServerUrl = computer.url;
        }

        if (!computerServerUrl) {
          // Last resort fallback: reconstruct from hostname (legacy behavior for cloud VMs)
          let hostName = '';
          if (isVM(computer)) {
            hostName =
              (computer as { host?: string }).host ||
              computer.vncUrl?.replace(/^https?:\/\//, '').split(/[:/]/)[0] ||
              '';
          } else if (isCustomComputer(computer)) {
            hostName = computer.url.replace(/^https?:\/\//, '').split(/[:/]/)[0] || '';
          }
          const protocol = hostName === 'localhost' || hostName === '127.0.0.1' ? 'http' : 'https';
          const port = '8443';
          computerServerUrl = `${protocol}://${hostName}:${port}`;
        }

        // Get inference config from adapter (for env vars like CUA_BASE_URL)
        const currentComputer = state.computers.find((c) => c.id === state.currentComputerId);
        const inferenceConfig = currentComputer
          ? await adapters.inference.getConfig(currentComputer)
          : { baseUrl: '', env: {} };

        // Build environment variables for the agent
        // The inference API URL goes in env, not as the agent server URL
        const env: Record<string, string> = { ...inferenceConfig.env };
        if (inferenceConfig.baseUrl) {
          env.CUA_BASE_URL = inferenceConfig.baseUrl;
        }
        if (inferenceConfig.apiKey) {
          env.CUA_API_KEY = inferenceConfig.apiKey;
        }

        const agentClient = new AgentClient(computerServerUrl, {
          apiKey: inferenceConfig.apiKey,
          timeout: 120000,
          retries: 3,
          signal: abortController.signal,
          env,
          onRetry: (attempt, maxRetries) => {
            chatDispatch({
              type: 'SET_RETRY_STATE',
              payload: { attempt, maxRetries },
            });
          },
        });

        if (shouldStopResponseRef.current) {
          const chatId = currentRequestRef.current?.chatId;
          if (chatId) setGlobalGenerating(chatId, false);
          chatDispatch({ type: 'SET_WAITING', payload: false });
          chatDispatch({ type: 'SET_RETRY_STATE', payload: null });
          currentRequestRef.current = null;
          return;
        }

        // Handle backward compatibility: add "cua/" prefix if not already present
        let modelId = model.id;
        if (!modelId.startsWith('cua/')) {
          modelId = `cua/${modelId}`;
        }

        const res = await agentClient.responses.create({
          model: modelId,
          input: messages,
          agent_kwargs: {
            use_prompt_caching: false,
            only_n_most_recent_images: 3,
          },
          env,
        });

        if (shouldStopResponseRef.current) {
          const chatId = currentRequestRef.current?.chatId;
          if (chatId) setGlobalGenerating(chatId, false);
          chatDispatch({ type: 'SET_WAITING', payload: false });
          chatDispatch({ type: 'SET_RETRY_STATE', payload: null });
          currentRequestRef.current = null;
          return;
        }

        // Increment iteration count
        if (currentRequestRef.current) {
          currentRequestRef.current.iterationCount =
            (currentRequestRef.current.iterationCount || 0) + 1;
        }

        // Check if response failed
        if (res?.status === 'failed' && res?.error) {
          trackTrajectoryFailed({ model, errorType: 'agent_error' });
          const chatId = currentRequestRef.current?.chatId;
          if (chatId) setGlobalGenerating(chatId, false);
          chatDispatch({ type: 'SET_WAITING', payload: false });
          chatDispatch({ type: 'SET_RETRY_STATE', payload: null });
          currentRequestRef.current = null;
          chatDispatch({
            type: 'SET_ERROR',
            payload: { message: res.error, timestamp: new Date() },
          });
          return;
        }

        let isRunning = true;
        if (res?.output && res.output.at(-1)?.type === 'message') {
          isRunning = false;
        }

        if (res?.output) {
          const newMessages = [...messages, ...res.output];
          const chatId = currentRequestRef.current?.chatId;

          if (currentRequestRef.current) {
            currentRequestRef.current.messages = newMessages;
          }

          // Update both local and global state
          if (chatId) {
            setMessages(chatId, newMessages);
            persistMessages(chatId, newMessages);
          }

          if (isRunning && !shouldStopResponseRef.current) {
            // Continue the conversation
            await sendAgentRequest(newMessages, abortController);
          } else {
            // Completed
            const chatId = currentRequestRef.current?.chatId;
            const iterationCount = currentRequestRef.current?.iterationCount || 0;
            const startTime = currentRequestRef.current?.startTime || Date.now();
            const durationMs = Date.now() - startTime;

            // Track trajectory completion
            trackTrajectoryCompleted({
              model,
              iterationCount,
              durationMs,
            });

            if (chatId) setGlobalGenerating(chatId, false);
            chatDispatch({ type: 'SET_WAITING', payload: false });
            chatDispatch({ type: 'SET_RETRY_STATE', payload: null });
            currentRequestRef.current = null;
          }
        } else {
          const chatId = currentRequestRef.current?.chatId;
          if (chatId) setGlobalGenerating(chatId, false);
          chatDispatch({ type: 'SET_WAITING', payload: false });
          chatDispatch({ type: 'SET_RETRY_STATE', payload: null });
          currentRequestRef.current = null;
        }
      } catch (error) {
        console.error('Agent request error:', error);
        const chatId = currentRequestRef.current?.chatId;
        if (chatId) setGlobalGenerating(chatId, false);
        chatDispatch({ type: 'SET_WAITING', payload: false });
        chatDispatch({ type: 'SET_RETRY_STATE', payload: null });

        const wasAborted =
          error instanceof Error &&
          (error.message.includes('aborted') || error.name === 'AbortError');

        if (wasAborted && abortReasonRef.current === 'manual') {
          trackTrajectoryStopped({ model });
          abortReasonRef.current = null;
          currentRequestRef.current = null;
          return;
        }

        let errorMessage = 'Unknown error';
        if (error instanceof Error) {
          if (wasAborted && abortReasonRef.current === 'timeout') {
            errorMessage = 'Request timed out.';
          } else if (wasAborted) {
            abortReasonRef.current = null;
            currentRequestRef.current = null;
            return;
          } else {
            errorMessage = error.message;
          }
        }

        trackTrajectoryFailed({ model, errorType: errorMessage });
        abortReasonRef.current = null;
        currentRequestRef.current = null;
        chatDispatch({
          type: 'SET_ERROR',
          payload: { message: errorMessage, timestamp: new Date() },
        });
      }
    },
    [
      // Note: chatState is accessed via chatStateRef to avoid stale closures
      state.computers,
      state.currentComputerId,
      adapters.inference,
      chatDispatch,
      playgroundDispatch,
      setGlobalGenerating,
      setMessages,
      persistMessages,
      trackTrajectoryCompleted,
      trackTrajectoryFailed,
      trackTrajectoryStopped,
    ]
  );

  const handleSendMessage = useCallback(async () => {
    const latestState = chatStateRef.current;
    const { currentInput, model, computer, messages, id: chatId } = latestState;

    if (!currentInput.trim()) return;
    if (!model) {
      console.warn('[useAgentRequest] No model selected. Chat state:', { chatId, model, computer });
      chatDispatch({
        type: 'SET_ERROR',
        payload: {
          message: 'Please select a model before sending a message.',
          timestamp: new Date(),
        },
      });
      return;
    }
    if (!computer) {
      console.warn('[useAgentRequest] No computer selected. Chat state:', {
        chatId,
        model,
        computer,
      });
      chatDispatch({
        type: 'SET_ERROR',
        payload: {
          message: 'Please select a sandbox before sending a message.',
          timestamp: new Date(),
        },
      });
      return;
    }

    shouldStopResponseRef.current = false;
    chatDispatch({ type: 'SET_ERROR', payload: null });

    const userMessage: UserMessage = {
      content: currentInput,
      role: 'user',
      type: 'message',
    };

    const updatedMessages = [...(messages || []), userMessage];

    // Update chat name if first message
    if ((messages?.length || 0) === 0) {
      const base = currentInput.trim().replace(/\s+/g, ' ');
      const maxLen = 60;
      const newName = base.length > maxLen ? `${base.slice(0, maxLen)}…` : base || 'New Chat';
      chatDispatch({ type: 'UPDATE_NAME', payload: newName });
      persistTitle(chatId, newName);
    }

    // Add user message and clear input
    setMessages(chatId, updatedMessages);
    chatDispatch({ type: 'CLEAR_INPUT' });
    persistMessages(chatId, updatedMessages);

    const abortController = new AbortController();
    currentRequestRef.current = {
      chatId,
      messages: updatedMessages,
      abortController,
      startTime: Date.now(),
      iterationCount: 0,
    };

    setGlobalGenerating(chatId, true);
    chatDispatch({ type: 'SET_WAITING', payload: true });

    // Track message sent for telemetry
    const isFirstMessage = (messages?.length || 0) === 0;
    trackMessageSent({
      model,
      isFirstMessage,
      sandboxType: isVM(computer) ? 'vm' : 'custom',
      message: currentInput,
    });

    await sendAgentRequest(updatedMessages, abortController);
  }, [
    chatDispatch,
    setMessages,
    setGlobalGenerating,
    persistMessages,
    persistTitle,
    sendAgentRequest,
    trackMessageSent,
  ]);

  const handleStopResponse = useCallback(() => {
    shouldStopResponseRef.current = true;
    abortReasonRef.current = 'manual';
    const chatId = currentRequestRef.current?.chatId;
    if (currentRequestRef.current?.abortController) {
      currentRequestRef.current.abortController.abort();
    }
    if (chatId) setGlobalGenerating(chatId, false);
    chatDispatch({ type: 'SET_WAITING', payload: false });
    chatDispatch({ type: 'SET_RETRY_STATE', payload: null });
    currentRequestRef.current = null;
  }, [chatDispatch, setGlobalGenerating]);

  const handleTimeout = useCallback(() => {
    shouldStopResponseRef.current = true;
    abortReasonRef.current = 'timeout';
    const chatId = currentRequestRef.current?.chatId;
    if (currentRequestRef.current?.abortController) {
      currentRequestRef.current.abortController.abort();
    }
    if (chatId) setGlobalGenerating(chatId, false);
    chatDispatch({ type: 'SET_WAITING', payload: false });
    currentRequestRef.current = null;
  }, [chatDispatch, setGlobalGenerating]);

  const handleRetry = useCallback(async () => {
    const latestState = chatStateRef.current;
    const { messages, id: chatId } = latestState;

    if (!messages || messages.length === 0) return;

    const abortController = new AbortController();
    currentRequestRef.current = {
      chatId,
      messages,
      abortController,
    };

    setGlobalGenerating(chatId, true);
    chatDispatch({ type: 'SET_WAITING', payload: true });
    await sendAgentRequest(messages, abortController);
  }, [chatDispatch, setGlobalGenerating, sendAgentRequest]);

  return {
    handleSendMessage,
    handleStopResponse,
    handleTimeout,
    handleRetry,
  };
}
