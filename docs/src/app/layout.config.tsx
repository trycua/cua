import type { BaseLayoutProps } from "fumadocs-ui/layouts/shared";

import Image from "next/image";
import LogoBlack from "@/assets/logo-black.svg";
import LogoWhite from "@/assets/logo-white.svg";
import DiscordWhite from "@/assets/discord-white.svg";
import DiscordBlack from "@/assets/discord-black.svg";
import { HomeIcon } from "lucide-react";

/**
 * Shared layout configurations
 *
 * you can customise layouts individually from:
 * Home Layout: app/(home)/layout.tsx
 * Docs Layout: app/docs/layout.tsx
 */
export const baseOptions: BaseLayoutProps = {
  nav: {
    title: (
      <>
        <Image
          width={30}
          height={30}
          src={LogoBlack}
          aria-label="Logo"
          className="block dark:hidden"
          alt="Logo"
        />
        <Image
          width={30}
          height={30}
          src={LogoWhite}
          aria-label="Logo"
          className="hidden dark:block"
          alt="Logo"
        />
        Cua Documentation
      </>
    ),
  },
  githubUrl: "https://github.com/trycua/cua",
  links: [
    {
      url: "https://trycua.com",
      text: "c/ua home",
      type: "icon",
      icon: <HomeIcon />,
    },
    {
      url: "https://discord.com/invite/mVnXXpdE85",
      text: "c/ua discord",
      type: "icon",
      icon: (
        <>
          <Image
            width={20}
            height={20}
            alt="Discord"
            className="hidden dark:block opacity-70 hover:opacity-100"
            src={DiscordWhite}
          />
          <Image
            width={20}
            height={20}
            alt="Discord"
            className="dark:hidden block opacity-55 hover:opacity-100"
            src={DiscordBlack}
          />
        </>
      ),
    },
  ],
};
