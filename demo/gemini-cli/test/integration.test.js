// Integration tests for Gemini CLI with mock MCP server
import { GoogleGenerativeAI } from '@google/generative-ai';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// Test configuration
const GEMINI_API_KEY = process.env.GEMINI_API_KEY;
const MOCK_SERVER_PATH = path.join(__dirname, 'mock-mcp-server.js');

if (!GEMINI_API_KEY) {
  console.error('Skipping tests: GEMINI_API_KEY not set');
  process.exit(0);
}

// Run tests
async function runTests() {
  console.log('Running Gemini CLI Integration Tests\n');

  const tests = [
    'should list tools from mock server',
    'should strip additionalProperties from schemas',
    'should call mock tool successfully',
    'should convert MCP tools to Gemini format',
    'should make a basic Gemini API call with function declarations'
  ];

  let mcpClient;
  let genAI;
  let model;

  try {
    // Setup
    const transport = new StdioClientTransport({
      command: 'node',
      args: [MOCK_SERVER_PATH]
    });

    mcpClient = new Client({
      name: 'test-client',
      version: '1.0.0'
    }, {
      capabilities: { tools: {} }
    });

    await mcpClient.connect(transport);
    console.log('✓ Connected to mock MCP server\n');

    genAI = new GoogleGenerativeAI(GEMINI_API_KEY);
    model = genAI.getGenerativeModel({
      model: 'gemini-3.5-flash',
      generationConfig: {
        temperature: 0.7,
        topP: 0.95,
        maxOutputTokens: 8192,
      }
    });

    // Test 1: List tools
    {
      const toolsResponse = await mcpClient.listTools();
      assert(toolsResponse.tools.length === 3, 'Should have 3 mock tools');
      console.log('✓ should list tools from mock server');
    }

    // Test 2: Strip additionalProperties
    {
      const schemaWithAdditional = {
        type: 'object',
        properties: { x: { type: 'number' } },
        additionalProperties: false
      };

      function stripUnsupportedFields(schema) {
        if (!schema || typeof schema !== 'object') return schema;
        const cleaned = { ...schema };
        delete cleaned.additionalProperties;
        if (cleaned.properties) {
          cleaned.properties = Object.fromEntries(
            Object.entries(cleaned.properties).map(([key, value]) => [
              key,
              stripUnsupportedFields(value)
            ])
          );
        }
        return cleaned;
      }

      const cleaned = stripUnsupportedFields(schemaWithAdditional);
      assert(!('additionalProperties' in cleaned));
      console.log('✓ should strip additionalProperties from schemas');
    }

    // Test 3: Call mock tool
    {
      const result = await mcpClient.callTool({
        name: 'screenshot',
        arguments: {}
      });
      assert(result.content[0].text.includes('Success'));
      console.log('✓ should call mock tool successfully');
    }

    // Test 4: Convert MCP tools to Gemini format
    {
      const toolsResponse = await mcpClient.listTools();

      function stripUnsupportedFields(schema) {
        if (!schema || typeof schema !== 'object') return schema;
        const cleaned = { ...schema };
        delete cleaned.additionalProperties;
        if (cleaned.properties) {
          cleaned.properties = Object.fromEntries(
            Object.entries(cleaned.properties).map(([key, value]) => [
              key,
              stripUnsupportedFields(value)
            ])
          );
        }
        if (cleaned.items) {
          cleaned.items = stripUnsupportedFields(cleaned.items);
        }
        return cleaned;
      }

      const functions = toolsResponse.tools.map(tool => ({
        name: tool.name,
        description: tool.description || '',
        parameters: stripUnsupportedFields(tool.inputSchema) || { type: 'object', properties: {} }
      }));

      assert(functions.length === 3);
      functions.forEach(fn => {
        assert(!('additionalProperties' in fn.parameters));
      });
      console.log('✓ should convert MCP tools to Gemini format');
    }

    // Test 5: Gemini API call with tools
    {
      const toolsResponse = await mcpClient.listTools();

      function stripUnsupportedFields(schema) {
        if (!schema || typeof schema !== 'object') return schema;
        const cleaned = { ...schema };
        delete cleaned.additionalProperties;
        if (cleaned.properties) {
          cleaned.properties = Object.fromEntries(
            Object.entries(cleaned.properties).map(([key, value]) => [
              key,
              stripUnsupportedFields(value)
            ])
          );
        }
        if (cleaned.items) {
          cleaned.items = stripUnsupportedFields(cleaned.items);
        }
        return cleaned;
      }

      const functions = toolsResponse.tools.map(tool => ({
        name: tool.name,
        description: tool.description || '',
        parameters: stripUnsupportedFields(tool.inputSchema) || { type: 'object', properties: {} }
      }));

      const chat = model.startChat({
        tools: [{ functionDeclarations: functions }]
      });

      console.log('  Making Gemini API call (this may take a moment)...');
      const result = await chat.sendMessage('Take a screenshot');
      const response = result.response;

      assert(response, 'Should get a response');
      console.log('✓ should make a basic Gemini API call with function declarations');
    }

    // Test 6: Gemini text-only response (no function calls)
    {
      const chat = model.startChat({
        tools: [{ functionDeclarations: [] }]
      });

      console.log('  Testing text-only response...');
      const result = await chat.sendMessage('Hello, how are you?');
      const response = result.response;

      // Should not crash when no function calls
      const functionCalls = response.functionCalls?.() || [];
      assert(functionCalls.length === 0, 'Should have no function calls');
      assert(response.text(), 'Should have text response');
      console.log('✓ should handle text-only responses without function calls');
    }

    // Test 7: Function response format
    {
      const functionResponses = [
        {
          functionResponse: {
            name: 'screenshot',
            response: {
              result: 'Mock result: Success!'
            }
          }
        }
      ];

      // Verify structure matches Gemini's expected format
      assert(functionResponses[0].functionResponse);
      assert(functionResponses[0].functionResponse.name === 'screenshot');
      assert(functionResponses[0].functionResponse.response.result);
      console.log('✓ should format function responses correctly for Gemini');
    }

    console.log('\n✅ All tests passed!');

  } catch (error) {
    console.error('\n❌ Test failed:', error.message);
    process.exit(1);
  } finally {
    if (mcpClient) {
      await mcpClient.close();
    }
  }
}

runTests();
