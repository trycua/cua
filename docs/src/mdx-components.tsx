import defaultMdxComponents from 'fumadocs-ui/mdx';
import * as TabsComponents from 'fumadocs-ui/components/tabs';
import type { MDXComponents } from 'mdx/types';
import { VideoDemo } from '@/components/video-demo';

// Local MDX preview: fumadocs defaults + Tabs (the only custom component the
// content uses). Hero/IOU/Mermaid/editable-code-block were unused by any page.
export function getMDXComponents(components?: MDXComponents): MDXComponents {
  return {
    ...defaultMdxComponents,
    ...TabsComponents,
    VideoDemo,
    ...components,
  };
}
