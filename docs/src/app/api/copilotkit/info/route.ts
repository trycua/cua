import { NextResponse } from 'next/server';

// Return agent info for the CopilotKit frontend
export const GET = async () => {
  return NextResponse.json({
    version: '1.0.0',
    agents: {
      default: {
        description: 'CUA Documentation Assistant - helps with CUA and CUA-Bench documentation questions',
      },
    },
  });
};
