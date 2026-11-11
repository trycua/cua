// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The small Markdown an agent's message uses: paragraphs, fenced code,
 * `inline code`, **bold**, links (https only) and @mentions. It is written to
 * be safe while text streams in: an unclosed fence or marker is drawn as it
 * is so far, never as an error and never reflowing earlier text.
 */

import { Linking, View } from "react-native";

import { useTheme } from "../../theme/theme";
import { Text } from "../../ui/Text";

export type Inline =
  | { t: "text"; v: string }
  | { t: "bold"; v: string }
  | { t: "code"; v: string }
  | { t: "link"; v: string; href: string }
  | { t: "mention"; v: string };
export type Block = { t: "p"; inline: Inline[] } | { t: "code"; v: string };

export function parseBlocks(src: string): Block[] {
  const out: Block[] = [];
  const lines = src.split("\n");
  let i = 0;
  let para: string[] = [];
  const flush = () => {
    if (para.length) out.push({ t: "p", inline: parseInline(para.join("\n")) });
    para = [];
  };
  while (i < lines.length) {
    const line = lines[i]!;
    if (/^\s*```/.test(line)) {
      flush();
      const body: string[] = [];
      i++;
      while (i < lines.length && !/^\s*```/.test(lines[i]!)) body.push(lines[i++]!);
      i++; // the closing fence, absent while streaming
      out.push({ t: "code", v: body.join("\n") });
      continue;
    }
    if (line.trim() === "") flush();
    else para.push(line);
    i++;
  }
  flush();
  return out;
}

const INLINE = /(\*\*[^*\n]+\*\*)|(`[^`\n]+`)|(\[[^\]\n]+\]\(https:\/\/[^)\s]+\))|(@[a-z0-9._-]+)/gi;

export function parseInline(s: string): Inline[] {
  const out: Inline[] = [];
  let last = 0;
  for (const m of s.matchAll(INLINE)) {
    if (m.index! > last) out.push({ t: "text", v: s.slice(last, m.index) });
    const tok = m[0];
    if (m[1]) out.push({ t: "bold", v: tok.slice(2, -2) });
    else if (m[2]) out.push({ t: "code", v: tok.slice(1, -1) });
    else if (m[3]) {
      const mm = /^\[([^\]]+)\]\((https:\/\/[^)\s]+)\)$/.exec(tok)!;
      out.push({ t: "link", v: mm[1]!, href: mm[2]! });
    } else out.push({ t: "mention", v: tok });
    last = m.index! + tok.length;
  }
  if (last < s.length) out.push({ t: "text", v: s.slice(last) });
  return out;
}

export function Markdown({ text, color }: { text: string; color: string }) {
  const { c, mono, scheme } = useTheme();
  const blocks = parseBlocks(text);
  const brand = scheme === "dark" ? c.brand : c.brandStrong;
  return (
    <View style={{ gap: 6 }}>
      {blocks.map((b, i) =>
        b.t === "code" ? (
          <View key={i} style={{ backgroundColor: c.background, borderRadius: 8, padding: 8 }} accessibilityLabel={`Code: ${b.v}`}>
            <Text variant="mono" style={{ color: c.foreground, fontFamily: mono }}>
              {b.v}
            </Text>
          </View>
        ) : (
          <Text key={i} variant="body" style={{ color }}>
            {b.inline.map((n, j) => {
              switch (n.t) {
                case "bold":
                  return (
                    <Text key={j} variant="body" weight="700" style={{ color }}>
                      {n.v}
                    </Text>
                  );
                case "code":
                  return <Text key={j} variant="body" style={{ color, fontFamily: mono, backgroundColor: c.background }}>{` ${n.v} `}</Text>;
                case "mention":
                  return (
                    <Text key={j} variant="body" weight="600" style={{ color: brand }}>
                      {n.v}
                    </Text>
                  );
                case "link":
                  return (
                    <Text
                      key={j}
                      variant="body"
                      style={{ color: brand, textDecorationLine: "underline" }}
                      onPress={() => void Linking.openURL(n.href)}
                      accessibilityRole="link"
                    >
                      {n.v}
                    </Text>
                  );
                default:
                  return n.v;
              }
            })}
          </Text>
        ),
      )}
    </View>
  );
}
