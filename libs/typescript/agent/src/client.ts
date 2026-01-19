import { Peer } from 'peerjs';
import { Telemetry } from '@trycua/core';
import { randomUUID } from 'node:crypto';
import type { AgentRequest, AgentResponse, ConnectionType, AgentClientOptions } from './types';

export class AgentClient {
  private url: string;
  private connectionType: ConnectionType;
  private options: AgentClientOptions;
  private peer?: Peer;
  private connection?: any;
  private telemetry: Telemetry;
  private sessionId: string;

  constructor(url: string, options: AgentClientOptions = {}) {
    this.url = url;
    this.options = {
      timeout: 30000,
      retries: 3,
      ...options,
    };
    this.telemetry = new Telemetry();
    this.sessionId = randomUUID();

    // Determine connection type from URL
    if (url.startsWith('http://') || url.startsWith('https://')) {
      this.connectionType = url.startsWith('https://') ? 'https' : 'http';
    } else if (url.startsWith('peer://')) {
      this.connectionType = 'peer';
    } else {
      throw new Error('Invalid URL format. Must start with http://, https://, or peer://');
    }

    // Emit agent_client_initialized event
    this.telemetry.recordEvent('agent_client_initialized', {
      session_id: this.sessionId,
      connection_type: this.connectionType,
      has_api_key: !!options.apiKey,
      node_version: process.version,
    });
  }

  // Main responses API matching the desired usage pattern
  public responses = {
    create: async (request: AgentRequest): Promise<AgentResponse> => {
      return this.sendRequest(request);
    },
  };

  private async sendRequest(request: AgentRequest): Promise<AgentResponse> {
    const requestId = randomUUID();
    const startTime = Date.now();

    // Emit agent_request_start event
    this.telemetry.recordEvent('agent_request_start', {
      session_id: this.sessionId,
      request_id: requestId,
      model: request.model,
      connection_type: this.connectionType,
      has_computer_kwargs: !!request.computer_kwargs,
      has_agent_kwargs: !!request.agent_kwargs,
    });

    try {
      let response: AgentResponse;
      switch (this.connectionType) {
        case 'http':
        case 'https':
          response = await this.sendHttpRequest(request);
          break;
        case 'peer':
          response = await this.sendPeerRequest(request);
          break;
        default:
          throw new Error(`Unsupported connection type: ${this.connectionType}`);
      }

      // Emit agent_request_end event on success
      this.telemetry.recordEvent('agent_request_end', {
        session_id: this.sessionId,
        request_id: requestId,
        model: request.model,
        duration_ms: Date.now() - startTime,
        status: response.status,
        num_messages: response.output?.length ?? 0,
        prompt_tokens: response.usage?.prompt_tokens ?? 0,
        completion_tokens: response.usage?.completion_tokens ?? 0,
        total_tokens: response.usage?.total_tokens ?? 0,
        response_cost: response.usage?.response_cost ?? 0,
      });

      return response;
    } catch (error) {
      // Emit agent_request_error event on failure
      const errorMessage = error instanceof Error ? error.message : 'Unknown error';
      const errorType = this.classifyError(errorMessage);

      this.telemetry.recordEvent('agent_request_error', {
        session_id: this.sessionId,
        request_id: requestId,
        model: request.model,
        duration_ms: Date.now() - startTime,
        error_type: errorType,
        error_message: errorMessage.slice(0, 200), // Truncate for safety
      });

      throw error;
    }
  }

  private classifyError(errorMessage: string): string {
    if (errorMessage.includes('timeout') || errorMessage.includes('abort')) {
      return 'timeout';
    }
    if (errorMessage.includes('HTTP error')) {
      return 'http_error';
    }
    if (errorMessage.includes('parse') || errorMessage.includes('JSON')) {
      return 'parse_error';
    }
    if (
      errorMessage.includes('network') ||
      errorMessage.includes('fetch') ||
      errorMessage.includes('connect')
    ) {
      return 'network';
    }
    if (errorMessage.includes('Peer')) {
      return 'peer_error';
    }
    return 'unknown';
  }

  private async sendHttpRequest(request: AgentRequest): Promise<AgentResponse> {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), this.options.timeout);

    try {
      const headers: Record<string, string> = {
        'Content-Type': 'application/json',
      };
      if (this.options.apiKey) {
        headers['X-API-Key'] = this.options.apiKey;
      }

      const response = await fetch(`${this.url}/responses`, {
        method: 'POST',
        headers,
        body: JSON.stringify(request),
        signal: controller.signal,
      });

      clearTimeout(timeoutId);

      if (!response.ok) {
        throw new Error(`HTTP error! status: ${response.status}`);
      }

      const data = await response.json();
      return data as AgentResponse;
    } catch (error) {
      clearTimeout(timeoutId);
      if (error instanceof Error) {
        throw new Error(`Failed to send HTTP request: ${error.message}`);
      }
      throw error;
    }
  }

  private async sendPeerRequest(request: AgentRequest): Promise<AgentResponse> {
    // Extract peer ID from peer:// URL
    const peerId = this.url.replace('peer://', '');

    if (!this.peer) {
      // Initialize peer connection with default options as requested
      this.peer = new Peer();

      return new Promise<AgentResponse>((resolve, reject) => {
        const timeout = setTimeout(() => {
          reject(new Error('Peer connection timeout'));
        }, this.options.timeout);

        this.peer!.on('open', () => {
          // Connect to the target peer
          this.connection = this.peer!.connect(peerId);

          this.connection.on('open', () => {
            // Send the request
            this.connection!.send(JSON.stringify(request));
          });

          this.connection.on('data', (data: any) => {
            clearTimeout(timeout);
            try {
              const response = typeof data === 'string' ? JSON.parse(data) : data;
              resolve(response as AgentResponse);
            } catch (error) {
              reject(new Error('Failed to parse peer response'));
            }
          });

          this.connection.on('error', (error: any) => {
            clearTimeout(timeout);
            reject(new Error(`Peer connection error: ${error}`));
          });
        });

        this.peer!.on('error', (error: any) => {
          clearTimeout(timeout);
          reject(new Error(`Peer error: ${error}`));
        });
      });
    } else {
      // Reuse existing connection
      return new Promise<AgentResponse>((resolve, reject) => {
        const timeout = setTimeout(() => {
          reject(new Error('Peer request timeout'));
        }, this.options.timeout);

        if (this.connection && this.connection.open) {
          this.connection.send(JSON.stringify(request));

          const handleData = (data: any) => {
            clearTimeout(timeout);
            this.connection!.off('data', handleData);
            try {
              const response = typeof data === 'string' ? JSON.parse(data) : data;
              resolve(response as AgentResponse);
            } catch (error) {
              reject(new Error('Failed to parse peer response'));
            }
          };

          this.connection.on('data', handleData);
        } else {
          clearTimeout(timeout);
          reject(new Error('Peer connection not available'));
        }
      });
    }
  }

  // Health check method
  async health(): Promise<{ status: string }> {
    if (this.connectionType === 'peer') {
      return { status: this.peer?.open ? 'connected' : 'disconnected' };
    }

    try {
      const response = await fetch(`${this.url}/health`);
      if (response.ok) {
        return { status: 'healthy' };
      }
      return { status: 'unhealthy' };
    } catch {
      return { status: 'unreachable' };
    }
  }

  // Clean up resources
  async disconnect(): Promise<void> {
    if (this.connection) {
      this.connection.close();
      this.connection = undefined;
    }
    if (this.peer) {
      this.peer.destroy();
      this.peer = undefined;
    }
    // Flush and shutdown telemetry
    await this.telemetry.shutdown();
  }
}
