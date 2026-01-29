# How We Built Our OSS Launch Video with Remotion + Claude Code

_Published on February 3, 2026 by the Cua Team_

We're in the middle of "5 Days of OSS Releases" at Cua—shipping one open-source project every day this week. For most of them, we put together clean infographics in Figma. But for Cua-Bench, our benchmarking tool, we wanted to try something different.

Then Tuesday happened. We're scrolling X and suddenly everyone's posting these "vibe coded" product videos—developers using AI to generate slick animated demos. We looked at each other like... we should probably try this.

Sarina, who leads our growth efforts, decided to dedicate some free cycles to figuring it out. Two hours later, we had a launch video. And honestly? It's better than our YC launch video. The one we paid $500 for on Fiverr last year. I'm still a little mad about that.

Anyway, here's everything we learned.

## Wait, What's Remotion?

Remotion is this cool library that lets you make videos with React. Instead of dragging stuff around in a timeline, you write code. Every frame is just... a function.

```typescript
const MyVideo = () => {
  const frame = useCurrentFrame();
  const opacity = interpolate(frame, [0, 30], [0, 1]);

  return <h1 style={{ opacity }}>Hello</h1>;
};
```

That's a fade-in. Text goes from invisible to visible over 30 frames. One second at 30fps.

For dev tool companies like us, this is kind of perfect. We're always showing code snippets, terminal commands, UI demos—stuff that's way easier to build in code than to animate in Premiere or After Effects. Plus when you inevitably need to change that one CLI command, you just edit a string and re-render. No hunting through timeline layers.

## The Thing That Made This Actually Work

Remotion just released these "skills" for Claude Code (shoutout to Vercel for making npx skills a thing):

```bash
npx skills add remotion-dev/skills
```

Basically it teaches Claude how Remotion works—the API, the patterns, the gotchas. Before this, Claude would write Remotion code that technically ran but looked janky. Animations would stutter, timing would be weird.

After adding the skills? Night and day. You describe what you want, Claude writes production-ready code.

Here's an actual example from our video. We wanted text to appear word by word with that satisfying staggered effect:

```typescript
const WORD_DATA = [
  { word: 'Computer-Use', appearFrame: 0 },
  { word: 'agents', appearFrame: 5 },
  { word: 'are', appearFrame: 14 },
  { word: 'closing', appearFrame: 20 },
  { word: 'the', appearFrame: 26 },
  { word: 'gap.', appearFrame: 31 },
];
```

Claude knew to use `Easing.out(Easing.cubic)` for smooth reveals. It knew to clamp the interpolation. These are things that take a while to figure out on your own—Claude just... knows them now.

## How We Set Up the Monorepo

We wanted anyone on the team to be able to make videos after this, not just the people who happened to learn Remotion this week. So we built a proper setup:

```
launchpad/
├── packages/
│   ├── shared/       # Animations everyone can use
│   └── assets/       # Brand stuff - colors, fonts, sounds
├── videos/
│   ├── _template/    # Copy this to start a new video
│   ├── cuabench/
│   └── [whatever's next]/
└── scripts/
    └── create-video.ts  # Makes new projects easy
```

The idea is simple:

**Shared components** — We made `FadeIn`, `SlideUp`, `TextReveal` components. You don't need to understand the math, just use them.

**Brand assets** — All our colors and fonts live in one place. Update it once, every video gets the update.

**Template** — New video? Copy the template folder and you're ready to go. Or run `pnpm create-video` and it asks you some questions.

We're using pnpm workspaces and Turbo for the build stuff. Nothing fancy, it just works.

## Things We Learned the Hard Way

### Put all your timing at the top

Seriously. Do this:

```typescript
const PHASE1_START = 0;
const PHASE1_END = 60;
const PHASE2_START = PHASE1_END;
const SCENE_DURATION = 180;
```

When someone says "make the intro longer" you change one number. Everything else adjusts. Don't hardcode frame numbers inside your JSX, you'll regret it.

### Make separate compositions for each scene

```typescript
<Composition id="IntroScene" component={IntroScene} durationInFrames={INTRO_DURATION} />
<Composition id="CodeEditorScene" component={CodeEditorScene} durationInFrames={EDITOR_DURATION} />
<Composition id="FullVideo" component={FullVideo} durationInFrames={FULL_DURATION} />
```

This way you can preview just the intro without sitting through the whole video every time. Trust me on this one.

### The interpolate pattern

You'll write this a hundred times:

```typescript
const progress = interpolate(frame, [START_FRAME, END_FRAME], [0, 1], {
  extrapolateLeft: 'clamp',
  extrapolateRight: 'clamp',
  easing: Easing.out(Easing.cubic),
});
```

Always clamp (otherwise things get weird at the edges). Always use easing for anything users will see. `Easing.out(Easing.cubic)` is our go-to—starts fast, lands smooth.

### Track your phases

When a scene has multiple things happening:

```typescript
const isPhase1 = frame < PHASE2_START;
const isPhase2 = frame >= PHASE2_START && frame < PHASE3_START;

return (
  <>
    {isPhase1 && <IntroContent />}
    {isPhase2 && <MainContent />}
  </>
);
```

Keeps the JSX readable. You can actually tell what's happening when.

## The Workflow We Settled On

It goes like this:

**1. Describe what you want**

"Make an intro that shows 'Computer-Use agents are closing the gap' with each word fading in one at a time. White text, dark background, Urbanist font."

**2. Claude writes the scene**

You get a complete component. Timing, animations, styling—all there.

**3. Preview it**

Run `pnpm remotion` to open Remotion Studio. Scrub through, see how it looks.

**4. Iterate**

"The words are appearing too slow, cut the gap between them in half"

Claude updates the code. Preview again. Repeat until it feels right.

Some tips that helped us:

- Be specific about timing. "Fade over 20 frames" is better than "fade in smoothly"
- Reference other scenes. "Style it like the intro" saves a lot of back and forth
- When you're learning, ask Claude to explain why it chose a particular easing or pattern

## Making This Work for the Whole Team

We wanted non-Remotion-experts to be able to make videos. Here's what we did:

**The template has everything pre-configured.** Remotion config, Next.js player for previews, a sample scene showing common patterns. You don't start from zero.

**The create script asks the right questions.** Project name, dimensions (1080p, 4K, square for social, whatever), and it sets everything up.

**Shared components hide the complexity.** Need a fade-in? Use `<FadeIn>`. Don't worry about interpolation.

The result: someone can describe a video to Claude Code, iterate on the output, and ship it—without ever reading the Remotion docs. That's the goal anyway. We'll see how it goes.

## What's Next

We're gonna clean up our monorepo and open-source it. The shared components, the template system, all of it. Keep an eye out.

Two hours. Zero video editing experience. A better result than the $500 Fiverr video. I'm still kind of in disbelief honestly.

If you're at a dev tool company and you're not doing this yet... you probably should be. Your videos become code. You can version them, review them, iterate on them. It's weirdly satisfying.

---

Shoutout to [@sarinajnli](https://x.com/sarinajnli) for running with this, to [@Remotion](https://x.com/Remotion) and to [@vercel](https://x.com/vercel) [@andrewqu](https://x.com/andrewqu) for npx skills. If you try something similar, let us know how it goes—[@trycua](https://x.com/trycua) on X.
