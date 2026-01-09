import { NextResponse } from 'next/server';

// Return agent info for the CopilotKit frontend (REST transport mode)
export const GET = async () => {
  return NextResponse.json({
    version: '1.50.1',
    agents: {},
    audioFileTranscriptionEnabled: false,
  });
};
