// Import the server SDK which works with Node.js (not browser-only like @vapi-ai/web)
import { VapiClient } from '@vapi-ai/server-sdk';

// Initialize Vapi client with your API key
const vapi = new VapiClient({
  apiKey: '1910ede1-e8f5-4c5e-aba2-deb03c53c9db'
});

console.log('🤖 Vapi server SDK initialized successfully!');

// Async function to handle API calls
async function testVapi() {
  // Example: Create a call using the server SDK
  // This is different from the web SDK - server SDK is for managing calls, not direct voice interaction
  try {
    const call = await vapi.calls.create({
      assistant: {
        id: 'a6210139-4abf-4ed8-b1a5-0996953045b3'
      },
      // For server SDK, you typically need to specify how to connect (phone, etc.)
      customer: {
        number: '+1234567890' // This would be the customer's phone number
      }
    });
    
    console.log('📞 Call created:', call.id);
  } catch (error) {
    console.log('ℹ️ Note: Server SDK is for managing calls, not direct voice interaction');
    console.log('For direct voice interaction, you need a web browser environment');
    console.log('Error details:', error.message);
  }
}

// Run the test
testVapi();