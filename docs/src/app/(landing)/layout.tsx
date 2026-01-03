import type { ReactNode } from 'react';
import { HomeLayout } from 'fumadocs-ui/layouts/home';
import { CustomHeader } from '@/components/custom-header';

export default function LandingLayout({ children }: { children: ReactNode }) {
  return (
    <>
      <CustomHeader />
      <HomeLayout nav={{ enabled: false }}>{children}</HomeLayout>
    </>
  );
}
