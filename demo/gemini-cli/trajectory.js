import fs from 'node:fs/promises';
import path from 'node:path';

export class TrajectoryRecorder {
  constructor(baseDir) {
    this.baseDir = baseDir;
    this.trajectoryDir = path.join(baseDir, 'trajectories');
    this.currentSession = null;
    this.messages = [];
    this.imageCounter = 0;
  }

  async startSession(sessionId, windowInfo) {
    this.currentSession = sessionId;
    this.messages = [];
    this.imageCounter = 0;
    this.windowInfo = windowInfo;

    await fs.mkdir(this.trajectoryDir, { recursive: true });
    await fs.mkdir(path.join(this.trajectoryDir, sessionId), { recursive: true });
    await fs.mkdir(path.join(this.trajectoryDir, sessionId, 'images'), { recursive: true });
  }

  async recordContent(content) {
    if (!this.currentSession) return;
    // Extract images from parts and save them
    const savedContent = {
      role: content.role,
      parts: await this._extractImages(content.parts)
    };
    this.messages.push(savedContent);
    await this._saveTrajectory();
  }

  async _extractImages(obj) {
    if (Array.isArray(obj)) {
      return Promise.all(obj.map(item => this._extractImages(item)));
    }

    if (obj && typeof obj === 'object') {
      const result = {};
      for (const [key, value] of Object.entries(obj)) {
        if (key === 'inlineData' && value.data) {
          // Save image and replace with path reference
          const imagePath = await this._saveImage(value.data, value.mimeType);
          result[key] = {
            mimeType: value.mimeType,
            _imagePath: imagePath
          };
        } else {
          result[key] = await this._extractImages(value);
        }
      }
      return result;
    }

    return obj;
  }

  async _saveImage(base64Data, mimeType) {
    const ext = mimeType === 'image/jpeg' ? 'jpg' : 'png';
    const filename = `img_${this.imageCounter++}.${ext}`;
    const imagePath = `images/${filename}`;

    const buffer = Buffer.from(base64Data, 'base64');
    await fs.writeFile(
      path.join(this.trajectoryDir, this.currentSession, imagePath),
      buffer
    );

    return imagePath;
  }

  async _saveTrajectory() {
    if (!this.currentSession) return;

    const trajectoryData = {
      session_id: this.currentSession,
      window: this.windowInfo,
      messages: this.messages
    };

    await fs.writeFile(
      path.join(this.trajectoryDir, this.currentSession, 'trajectory.json'),
      JSON.stringify(trajectoryData, null, 2)
    );
  }

  async exportMinimalRepro(sessionId) {
    const sid = sessionId || this.currentSession;
    const sessionDir = path.join(this.trajectoryDir, sid);
    const trajectoryPath = path.join(sessionDir, 'trajectory.json');

    const data = JSON.parse(await fs.readFile(trajectoryPath, 'utf-8'));
    const exportDir = path.join(sessionDir, 'export');

    await fs.mkdir(exportDir, { recursive: true });
    await fs.mkdir(path.join(exportDir, 'images'), { recursive: true });

    // Copy all images
    const imagesDir = path.join(sessionDir, 'images');
    const imageFiles = await fs.readdir(imagesDir);
    for (const file of imageFiles) {
      await fs.copyFile(
        path.join(imagesDir, file),
        path.join(exportDir, 'images', file)
      );
    }

    // Generate minimal Python script
    const pythonScript = generateMinimalPython(data);
    await fs.writeFile(path.join(exportDir, 'repro.py'), pythonScript);

    console.log(`\n✓ Export created: ${exportDir}`);
    console.log('  - repro.py');
    console.log(`  - images/ (${imageFiles.length} images)\n`);

    return exportDir;
  }

  getCurrentSession() {
    return this.currentSession;
  }
}

function itemToPython(item, indent = 2) {
  const ind = '    '.repeat(indent);

  if (item.inlineData) {
    const imgPath = item.inlineData._imagePath;
    const mimeType = item.inlineData.mimeType;
    return `${ind}Part(inline_data={"mime_type": "${mimeType}", "data": load_img("${imgPath}")})`;
  } else if (item.text) {
    return `${ind}Part(text=${JSON.stringify(item.text)})`;
  } else if (item.functionResponse) {
    const name = item.functionResponse.name;
    const resp = item.functionResponse.response;

    if (resp.inlineData) {
      const imgPath = resp.inlineData._imagePath;
      const mimeType = resp.inlineData.mimeType;
      return `${ind}Part(function_response={"name": "${name}", "response": {"inline_data": {"mime_type": "${mimeType}", "data": load_img("${imgPath}")}}})`;
    } else {
      const respJson = JSON.stringify(resp).replace(/true/g, 'True').replace(/false/g, 'False').replace(/null/g, 'None');
      return `${ind}Part(function_response={"name": "${name}", "response": ${respJson}})`;
    }
  }
}

function generateMinimalPython(data) {
  const lines = [];

  lines.push('#!/usr/bin/env -S uv run --script');
  lines.push('# /// script');
  lines.push('# requires-python = ">=3.11"');
  lines.push('# dependencies = [');
  lines.push('#   "google-genai>=2.8.0",');
  lines.push('# ]');
  lines.push('# ///');
  lines.push('"""Minimal repro - replays exact Gemini API calls"""');
  lines.push('import os, base64, warnings');
  lines.push('from google import genai');
  lines.push('from google.genai.types import Content, Part, GenerateContentConfig, Tool, ComputerUse, Environment');
  lines.push('');
  lines.push('# Suppress SDK warnings about non-text parts');
  lines.push('warnings.filterwarnings("ignore", message=".*non-text parts.*")');
  lines.push('');
  lines.push('client = genai.Client(api_key=os.environ["GEMINI_API_KEY"])');
  lines.push('');
  lines.push('def load_img(p):');
  lines.push('    with open(p, "rb") as f:');
  lines.push('        return base64.b64encode(f.read()).decode()');
  lines.push('');

  // Build conversation array with Content objects
  lines.push('conversation = [');
  const contents = [];
  for (const msg of data.messages) {
    const parts = msg.parts.map(part => {
      if (part.inlineData) {
        const imgPath = part.inlineData._imagePath;
        const mimeType = part.inlineData.mimeType;
        return `        Part(inline_data={"mime_type": "${mimeType}", "data": load_img("${imgPath}")})`;
      } else if (part.text !== undefined) {
        return `        Part(text=${JSON.stringify(part.text)})`;
      } else if (part.functionCall) {
        // Build the Part with function_call and optional thought_signature
        const fcJson = JSON.stringify(part.functionCall).replace(/true/g, 'True').replace(/false/g, 'False').replace(/null/g, 'None');
        if (part.thoughtSignature) {
          const tsJson = JSON.stringify(part.thoughtSignature);
          return `        Part(function_call=${fcJson}, thought_signature=${tsJson})`;
        } else {
          return `        Part(function_call=${fcJson})`;
        }
      } else if (part.functionResponse) {
        const name = part.functionResponse.name;
        const resp = part.functionResponse.response;
        if (resp.inlineData) {
          const imgPath = resp.inlineData._imagePath;
          const mimeType = resp.inlineData.mimeType;
          return `        Part(function_response={"name": "${name}", "response": {"inline_data": {"mime_type": "${mimeType}", "data": load_img("${imgPath}")}}})`;
        } else {
          const respJson = JSON.stringify(resp).replace(/true/g, 'True').replace(/false/g, 'False').replace(/null/g, 'None');
          return `        Part(function_response={"name": "${name}", "response": ${respJson}})`;
        }
      }
    }).filter(Boolean).join(',\n');

    contents.push(`    Content(role="${msg.role}", parts=[\n${parts}\n    ])`);
  }
  lines.push(contents.join(',\n'));
  lines.push(']');
  lines.push('');

  // Print conversation
  lines.push('def show_convo(history):');
  lines.push('    for i, content in enumerate(history):');
  lines.push('        role = content.role');
  lines.push('        print(f"[{i}] {role}:")');
  lines.push('        for part in content.parts:');
  lines.push('            # Check which type of part this is by seeing what\'s not None');
  lines.push('            text = getattr(part, "text", None)');
  lines.push('            inline_data = getattr(part, "inline_data", None)');
  lines.push('            function_call = getattr(part, "function_call", None)');
  lines.push('            function_response = getattr(part, "function_response", None)');
  lines.push('            ');
  lines.push('            if text is not None:');
  lines.push('                print(f"    text: {text[:80]}")');
  lines.push('            elif inline_data is not None:');
  lines.push('                mime = getattr(inline_data, "mime_type", "unknown")');
  lines.push('                print(f"    image: {mime}")');
  lines.push('            elif function_call is not None:');
  lines.push('                name = getattr(function_call, "name", "unknown")');
  lines.push('                args = getattr(function_call, "args", {})');
  lines.push('                print(f"    fn_call: {name}({dict(args) if args else {}})")');
  lines.push('            elif function_response is not None:');
  lines.push('                name = getattr(function_response, "name", "unknown")');
  lines.push('                print(f"    fn_resp: {name}")');
  lines.push('');
  lines.push('print("=== Full Conversation ===")');
  lines.push('show_convo(conversation)');
  lines.push('');
  lines.push('print("\\n=== Replaying ===")');
  lines.push('');
  lines.push('for i, content in enumerate(conversation):');
  lines.push('    # Only replay at model turns - send history UP TO this point');
  lines.push('    if content.role == "model":');
  lines.push('        print(f"\\n--- Replay at turn [{i}] ---")');
  lines.push('        # Send conversation history without this model turn to see what Gemini generates');
  lines.push('        resp = client.models.generate_content(');
  lines.push('            model="gemini-3.5-flash",');
  lines.push('            contents=conversation[:i],');
  lines.push('            config=GenerateContentConfig(');
  lines.push('                tools=[Tool(computer_use=ComputerUse(environment=Environment.ENVIRONMENT_DESKTOP))]');
  lines.push('            )');
  lines.push('        )');
  lines.push('        # Show model response');
  lines.push('        if resp.text:');
  lines.push('            print(f"Text: {resp.text}")');
  lines.push('        if hasattr(resp, "candidates") and resp.candidates:');
  lines.push('            parts = resp.candidates[0].content.parts if resp.candidates[0].content else []');
  lines.push('            for part in parts:');
  lines.push('                if hasattr(part, "function_call") and part.function_call:');
  lines.push('                    fc = part.function_call');
  lines.push('                    args = {k: v for k, v in dict(fc.args).items() if k != "intent"}');
  lines.push('                    intent = dict(fc.args).get("intent", "")');
  lines.push('                    if intent:');
  lines.push('                        print(f"💭 {intent}")');
  lines.push('                    print(f"🔧 {fc.name} {args}")');
  lines.push('        print()');

  return lines.join('\n');
}
