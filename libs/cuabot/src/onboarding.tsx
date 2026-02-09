#!/usr/bin/env node
/**
 * CuaBot Onboarding UI
 */

import React, { useState, useEffect } from 'react';
import { render, Box, Text, useInput, useApp } from 'ink';
import {
  checkDocker,
  checkXpra,
  checkPlaywright,
  checkDockerImage,
  pullDockerImage,
} from './utils.js';
import {
  AGENTS,
  AgentId,
  getDefaultAgent,
  setDefaultAgent,
  setTelemetryEnabled,
  loadSettings,
  getAliasIgnored,
  setAliasIgnored,
} from './settings.js';
import { exec, execSync } from 'child_process';
import { homedir } from 'os';
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'fs';
import { join } from 'path';

type CheckStatus = 'ok' | 'error' | 'loading';

interface Check {
  label: string;
  status: CheckStatus;
  message: string;
}

function checkCuabotInPath(): boolean {
  try {
    const cmd = process.platform === 'win32' ? 'where cuabot' : 'which cuabot';
    const result = execSync(cmd, { encoding: 'utf-8' }).trim();
    // Ignore paths from npx/pnpm dlx temporary cache
    const paths = result
      .split(/\r?\n/)
      .filter((p) => !p.includes('_npx') && !p.includes('\\dlx\\') && !p.includes('/dlx/'));
    return paths.length > 0;
  } catch {
    return false;
  }
}

function getShellRcFile(): string | null {
  if (process.platform === 'win32') {
    // PowerShell profile paths
    const ps7Profile = join(
      homedir(),
      'Documents',
      'PowerShell',
      'Microsoft.PowerShell_profile.ps1'
    );
    const ps5Profile = join(
      homedir(),
      'Documents',
      'WindowsPowerShell',
      'Microsoft.PowerShell_profile.ps1'
    );
    // Prefer PS7 if its directory exists, otherwise PS5
    if (existsSync(join(homedir(), 'Documents', 'PowerShell'))) return ps7Profile;
    return ps5Profile;
  }

  const shell = process.env.SHELL || '';
  if (shell.includes('zsh')) return join(homedir(), '.zshrc');
  if (shell.includes('bash')) {
    // macOS uses .bash_profile, Linux uses .bashrc
    const bashProfile = join(homedir(), '.bash_profile');
    if (process.platform === 'darwin' && existsSync(bashProfile)) return bashProfile;
    return join(homedir(), '.bashrc');
  }
  return null;
}

function addAliasToShell(): boolean {
  const isWindows = process.platform === 'win32';

  if (isWindows) {
    return addWindowsAlias();
  }

  const rcFile = getShellRcFile();
  if (!rcFile) return false;

  try {
    // Check if alias already exists
    if (existsSync(rcFile)) {
      const content = readFileSync(rcFile, 'utf-8');
      if (content.includes('alias cuabot=')) {
        setAliasIgnored(true);
        return true;
      }
    }
    appendFileSync(rcFile, '\n# cuabot alias\nalias cuabot="npx -y cuabot"\n');
    setAliasIgnored(true);
    return true;
  } catch {
    return false;
  }
}

function addWindowsAlias(): boolean {
  let success = false;

  // 1. Add PowerShell function to profile
  const psProfile = getShellRcFile();
  if (psProfile) {
    try {
      const dir = join(psProfile, '..');
      if (!existsSync(dir)) {
        mkdirSync(dir, { recursive: true });
      }

      const needsAdd =
        !existsSync(psProfile) || !readFileSync(psProfile, 'utf-8').includes('function cuabot');
      if (needsAdd) {
        appendFileSync(psProfile, '\n# cuabot function\nfunction cuabot { npx -y cuabot @args }\n');
      }
      success = true;
    } catch {
      // Continue to try batch file
    }
  }

  // 2. Create batch file for cmd.exe in WindowsApps (already in PATH)
  const windowsApps = join(
    process.env.LOCALAPPDATA || join(homedir(), 'AppData', 'Local'),
    'Microsoft',
    'WindowsApps'
  );
  const batchFile = join(windowsApps, 'cuabot.cmd');
  try {
    if (!existsSync(batchFile)) {
      writeFileSync(batchFile, '@echo off\r\nnpx -y cuabot %*\r\n');
    }
    success = true;
  } catch {
    // Batch file creation failed, but PowerShell might have worked
  }

  if (success) {
    setAliasIgnored(true);
  }
  return success;
}

function StatusLine({ check }: { check: Check }) {
  const icon = check.status === 'ok' ? '✓' : check.status === 'error' ? '✗' : '○';
  const color = check.status === 'ok' ? 'green' : check.status === 'error' ? 'red' : 'gray';
  return (
    <Box>
      <Text color={color}>{icon} </Text>
      <Text>{check.label}: </Text>
      <Text dimColor>{check.message}</Text>
    </Box>
  );
}

function AgentSelector({
  onSelect,
  onBack,
}: {
  onSelect: (agent: string) => void;
  onBack: () => void;
}) {
  const agentList = Object.entries(AGENTS) as [AgentId, (typeof AGENTS)[AgentId]][];
  const [selectedIndex, setSelectedIndex] = useState(0);

  useInput((input, key) => {
    if (key.upArrow) {
      setSelectedIndex((i) => (i > 0 ? i - 1 : agentList.length - 1));
    } else if (key.downArrow) {
      setSelectedIndex((i) => (i < agentList.length - 1 ? i + 1 : 0));
    } else if (key.return) {
      onSelect(agentList[selectedIndex][0]);
    } else if (key.escape) {
      onBack();
    }
  });

  return (
    <Box flexDirection="column" marginTop={1}>
      <Text dimColor>Select default agent (↑↓ Enter):</Text>
      <Box flexDirection="column" marginTop={1}>
        {agentList.map(([id, agent], index) => (
          <Box key={id}>
            <Text color={index === selectedIndex ? 'cyan' : undefined}>
              {index === selectedIndex ? '❯ ' : '  '}
              {agent.name}
            </Text>
            <Text dimColor> - {agent.description}</Text>
          </Box>
        ))}
      </Box>
    </Box>
  );
}

function OptionSelector({
  options,
  onSelect,
}: {
  options: { label: string; action: () => void }[];
  onSelect: () => void;
}) {
  const [selectedIndex, setSelectedIndex] = useState(0);

  useInput((input, key) => {
    if (key.upArrow) {
      setSelectedIndex((i) => (i > 0 ? i - 1 : options.length - 1));
    } else if (key.downArrow) {
      setSelectedIndex((i) => (i < options.length - 1 ? i + 1 : 0));
    } else if (key.return) {
      options[selectedIndex].action();
      onSelect();
    }
  });

  return (
    <Box flexDirection="column" marginTop={1}>
      {options.map((opt, index) => (
        <Box key={index}>
          <Text color={index === selectedIndex ? 'cyan' : undefined}>
            {index === selectedIndex ? '❯ ' : '  '}
            {opt.label}
          </Text>
        </Box>
      ))}
    </Box>
  );
}

function TelemetrySelector({ onSelect }: { onSelect: (enabled: boolean) => void }) {
  const [selectedIndex, setSelectedIndex] = useState(0);
  const options = [
    {
      label: 'Yes, share prompts and usage data',
      description: 'Help improve computer-use technology',
    },
    { label: 'No thanks', description: 'Keep prompts private' },
  ];

  useInput((input, key) => {
    if (key.upArrow) {
      setSelectedIndex((i) => (i > 0 ? i - 1 : options.length - 1));
    } else if (key.downArrow) {
      setSelectedIndex((i) => (i < options.length - 1 ? i + 1 : 0));
    } else if (key.return) {
      onSelect(selectedIndex === 0);
    }
  });

  return (
    <Box flexDirection="column" marginTop={1}>
      <Text dimColor>Share prompts to help improve computer-use technology?</Text>
      <Box flexDirection="column" marginTop={1}>
        {options.map((opt, index) => (
          <Box key={index}>
            <Text color={index === selectedIndex ? 'cyan' : undefined}>
              {index === selectedIndex ? '❯ ' : '  '}
              {opt.label}
            </Text>
          </Box>
        ))}
      </Box>
    </Box>
  );
}

function Onboarding() {
  const { exit } = useApp();
  const [checks, setChecks] = useState<Check[]>([
    { label: 'Default Agent', status: 'loading', message: 'checking...' },
    { label: 'cuabot Command', status: 'loading', message: 'checking...' },
    { label: 'Docker', status: 'loading', message: 'checking...' },
    { label: 'Docker Image', status: 'loading', message: 'checking...' },
    { label: 'Xpra Client', status: 'loading', message: 'checking...' },
    { label: 'Playwright', status: 'loading', message: 'checking...' },
    { label: 'Usage Telemetry', status: 'loading', message: 'checking...' },
  ]);
  const [pullingImage, setPullingImage] = useState(false);
  const [pullProgress, setPullProgress] = useState('');
  const [loading, setLoading] = useState(true);
  const [showAgentSelector, setShowAgentSelector] = useState(false);
  const [showTelemetrySelector, setShowTelemetrySelector] = useState(false);
  const [firstError, setFirstError] = useState<string | null>(null);
  const [xpraQuarantined, setXpraQuarantined] = useState(false);

  const runChecks = async () => {
    setLoading(true);
    const newChecks: Check[] = [];

    // Check default agent
    const defaultAgent = getDefaultAgent();
    newChecks.push({
      label: 'Default Agent',
      status: defaultAgent ? 'ok' : 'error',
      message: defaultAgent
        ? `${AGENTS[defaultAgent as AgentId]?.name || defaultAgent}`
        : 'not configured',
    });

    // Check cuabot command availability
    const cuabotInPath = checkCuabotInPath();
    const aliasIgnored = getAliasIgnored();
    newChecks.push({
      label: 'cuabot Command',
      status: cuabotInPath || aliasIgnored ? 'ok' : 'error',
      message: cuabotInPath ? 'ready' : aliasIgnored ? 'using npx' : 'not set up',
    });

    // Check Docker
    const docker = await checkDocker();
    newChecks.push({
      label: 'Docker',
      status: docker.ok ? 'ok' : 'error',
      message: docker.ok ? 'running' : 'not found',
    });

    // Check Docker Image (only if Docker is running)
    if (docker.ok) {
      const dockerImage = await checkDockerImage();
      newChecks.push({
        label: 'Docker Image',
        status: dockerImage.ok ? 'ok' : 'error',
        message: dockerImage.message,
      });
    } else {
      newChecks.push({
        label: 'Docker Image',
        status: 'error',
        message: 'requires Docker',
      });
    }

    // Check Xpra
    const xpra = await checkXpra();
    setXpraQuarantined(xpra.quarantined || false);
    newChecks.push({
      label: 'Xpra Client',
      status: xpra.ok ? 'ok' : 'error',
      message: xpra.message,
    });

    // Check Playwright
    const playwright = await checkPlaywright();
    newChecks.push({
      label: 'Playwright',
      status: playwright.ok ? 'ok' : 'error',
      message: playwright.message,
    });

    // Check telemetry
    const settings = loadSettings();
    const telemetryAsked = settings.telemetryEnabled !== undefined;
    newChecks.push({
      label: 'Usage Telemetry',
      status: telemetryAsked ? 'ok' : 'error',
      message: telemetryAsked
        ? settings.telemetryEnabled
          ? 'enabled'
          : 'disabled'
        : 'not configured',
    });

    setChecks(newChecks);
    setLoading(false);

    // Find first error
    const first = newChecks.find((c) => c.status === 'error');
    setFirstError(first?.label || null);

    // If all good, exit
    if (newChecks.every((c) => c.status === 'ok')) {
      console.log("\n✓ Ready! Run 'cuabot' to start.\n");
      exit();
    }
  };

  useEffect(() => {
    runChecks();
  }, []);

  useInput((input, key) => {
    if (key.escape || (key.ctrl && input === 'c')) {
      exit();
    }
  });

  const openUrl = (url: string) => {
    const cmd =
      process.platform === 'darwin' ? 'open' : process.platform === 'win32' ? 'start' : 'xdg-open';
    exec(`${cmd} "${url}"`);
  };

  const copyToClipboard = (text: string) => {
    const cmd =
      process.platform === 'darwin'
        ? 'pbcopy'
        : process.platform === 'win32'
          ? 'clip'
          : 'xclip -selection clipboard';
    exec(`printf '%s' "${text}" | ${cmd}`);
  };

  const getXpraUrl = () => {
    if (process.platform === 'darwin')
      return 'https://github.com/Xpra-org/xpra/wiki/Download#-macos';
    if (process.platform === 'linux')
      return 'https://github.com/Xpra-org/xpra/wiki/Download#-linux';
    return 'https://github.com/Xpra-org/xpra/wiki/Download#full-builds';
  };

  if (loading) {
    return (
      <Box flexDirection="column" padding={1}>
        <Text dimColor>cuabot onboarding</Text>
        <Text>Checking...</Text>
      </Box>
    );
  }

  if (showAgentSelector) {
    return (
      <Box flexDirection="column" padding={1}>
        <Text dimColor>cuabot onboarding</Text>
        <Box flexDirection="column" marginTop={1}>
          {checks.map((check, i) => (
            <StatusLine key={i} check={check} />
          ))}
        </Box>
        <AgentSelector
          onSelect={(agent) => {
            setDefaultAgent(agent);
            setShowAgentSelector(false);
            runChecks();
          }}
          onBack={() => setShowAgentSelector(false)}
        />
      </Box>
    );
  }

  if (showTelemetrySelector) {
    return (
      <Box flexDirection="column" padding={1}>
        <Text dimColor>cuabot onboarding</Text>
        <Box flexDirection="column" marginTop={1}>
          {checks.map((check, i) => (
            <StatusLine key={i} check={check} />
          ))}
        </Box>
        <TelemetrySelector
          onSelect={(enabled) => {
            setTelemetryEnabled(enabled);
            setShowTelemetrySelector(false);
            runChecks();
          }}
        />
      </Box>
    );
  }

  return (
    <Box flexDirection="column" padding={1}>
      <Text dimColor>cuabot onboarding</Text>
      <Box flexDirection="column" marginTop={1}>
        {checks.map((check, i) => (
          <StatusLine key={i} check={check} />
        ))}
      </Box>

      {firstError === 'Default Agent' && (
        <OptionSelector
          options={[{ label: 'Configure default agent', action: () => setShowAgentSelector(true) }]}
          onSelect={() => {}}
        />
      )}

      {firstError === 'Usage Telemetry' && (
        <OptionSelector
          options={[
            { label: 'Configure usage telemetry', action: () => setShowTelemetrySelector(true) },
          ]}
          onSelect={() => {}}
        />
      )}

      {firstError === 'Docker' && (
        <OptionSelector
          options={[
            {
              label: 'Open Docker Download',
              action: () => openUrl('https://www.docker.com/products/docker-desktop/'),
            },
            { label: 'Check Again', action: () => runChecks() },
          ]}
          onSelect={() => {}}
        />
      )}

      {firstError === 'Docker Image' && !pullingImage && (
        <OptionSelector
          options={[
            {
              label: 'Pull Docker image (~2GB)',
              action: async () => {
                setPullingImage(true);
                setPullProgress('Starting pull...');
                await pullDockerImage((line) => setPullProgress(line));
                setPullingImage(false);
                runChecks();
              },
            },
            { label: 'Check Again', action: () => runChecks() },
          ]}
          onSelect={() => {}}
        />
      )}

      {firstError === 'Docker Image' && pullingImage && (
        <Box flexDirection="column" marginTop={1}>
          <Text color="cyan">Pulling Docker image...</Text>
          <Text dimColor>{pullProgress}</Text>
        </Box>
      )}

      {firstError === 'Xpra Client' && !xpraQuarantined && (
        <OptionSelector
          options={[
            { label: 'Open Xpra Download', action: () => openUrl(getXpraUrl()) },
            { label: 'Check Again', action: () => runChecks() },
          ]}
          onSelect={() => {}}
        />
      )}

      {firstError === 'Xpra Client' && xpraQuarantined && (
        <OptionSelector
          options={[
            {
              label: 'Exit and copy command: \x1b[2msudo xattr -c /Applications/Xpra.app\x1b[0m',
              action: () => {
                copyToClipboard('sudo xattr -c /Applications/Xpra.app');
                exit();
              },
            },
            {
              label: 'Read why',
              action: () => openUrl('https://github.com/Xpra-org/xpra/wiki/Download#-macos'),
            },
            { label: 'Exit', action: () => exit() },
          ]}
          onSelect={() => {}}
        />
      )}

      {firstError === 'Playwright' && (
        <OptionSelector
          options={[
            {
              label: 'Exit and copy command: \x1b[2mnpx playwright install\x1b[0m',
              action: () => {
                copyToClipboard('npx playwright install');
                exit();
              },
            },
            { label: 'Exit', action: () => exit() },
          ]}
          onSelect={() => {}}
        />
      )}

      {firstError === 'cuabot Command' && (
        <OptionSelector
          options={[
            {
              label: `Set up 'cuabot' command`,
              action: () => {
                if (addAliasToShell()) {
                  if (process.platform === 'win32') {
                    console.log("\n✓ Added! Restart your terminal to use 'cuabot' command.\n");
                  } else {
                    console.log(
                      '\n✓ Added! Restart your terminal or run: source ' + getShellRcFile() + '\n'
                    );
                  }
                }
                runChecks();
              },
            },
            {
              label: "Skip (use 'npx cuabot' instead)",
              action: () => {
                setAliasIgnored(true);
                runChecks();
              },
            },
          ]}
          onSelect={() => {}}
        />
      )}
    </Box>
  );
}

export function runOnboarding() {
  render(<Onboarding />);
}
