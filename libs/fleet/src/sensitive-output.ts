export interface UserApiKeyCredentials {
  clientId: string
  clientSecret: string
  tokenUrl: string
  name: string
  scope: string[]
}

export type SensitiveOutput = {
  kind: "user_api_key"
  value: UserApiKeyCredentials
}

export class SensitiveOutputBuffer {
  private outputs: SensitiveOutput[] = []

  push(output: SensitiveOutput): void {
    this.outputs.push(output)
  }

  drain(): SensitiveOutput[] {
    const outputs = this.outputs
    this.outputs = []
    return outputs
  }
}
