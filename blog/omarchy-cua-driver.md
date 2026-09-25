# Cua Driver in Omarchy: A New Foundation for Computer Use

_Published on September 25, 2026 by Francesco Bonacci_

Today we're announcing the stable Cua Driver release for Omarchy - a new foundation for computer use, built into the OS from the ground up.

Over the last month, we worked directly with [David Heinemeier Hansson](https://x.com/dhh) and [Spencer Bull](https://x.com/SpencerGBull) to bring a native synthetic cursor to Omarchy's [Hyprland compositor](https://hyprland.org/), enabling true multi-cursor computer use at the OS level.

![Omarchy desktop with separate user and agent cursors](./assets/omarchy-cua-driver/slide-01.png)

## Computer use built into the OS

Most desktop automation has to borrow the user's visible pointer or take over a remote session. That makes it difficult for an agent to work while a person continues using the computer.

Omarchy gives Cua Driver a different foundation: a synthetic cursor that exists inside the compositor. The agent can operate a background window while your own pointer remains available for the work in front of you.

## A cursor that belongs to the compositor

The integration has three parts:

1. Cua Driver chooses where to point, click, or type.
2. The Omarchy Hyprland plugin draws and routes a second synthetic cursor.
3. Applications receive the resulting pointer input through the compositor.

This keeps the agent's input path separate from the user's pointer without pretending that the agent has taken over the whole desktop.

![A synthetic cursor from the compositor up](./assets/omarchy-cua-driver/slide-02.png)

## Your pointer stays yours

With a separate agent cursor, you can keep working in the window in front while an agent works in a background window. The two activities have distinct pointers and distinct destinations.

That is the practical difference between background computer use and software that simply moves your visible mouse around. The compositor owns both cursors and can route each one to the right window.

![User and agent cursors working in separate windows](./assets/omarchy-cua-driver/slide-03.png)

## What is available today

The current Omarchy Cua integration is available through the Omarchy Edge channel. Stable-channel promotion is not independently verified in this post.

Cua Driver publishes Linux ARM64 artifacts. Native Omarchy ARM production packaging and certification are separate work and are not signed off yet.

![Current Omarchy and ARM status](./assets/omarchy-cua-driver/slide-04.png)

## Try it on Omarchy

To try the current integration, use the Omarchy Edge channel and explore the projects behind it:

- [Omarchy](https://omarchy.org/)
- [Cua Driver](https://cua.ai/)
- [Cua source](https://github.com/trycua/cua)
- [Omarchy source](https://github.com/omacom/omarchy)

This is background computer use with a compositor-native synthetic cursor. It is not a benchmark result or a claim of full ARM production support.

![Try background computer use on Omarchy Edge](./assets/omarchy-cua-driver/slide-05.png)

