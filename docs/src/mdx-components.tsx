import defaultMdxComponents from 'fumadocs-ui/mdx';
import * as TabsComponents from 'fumadocs-ui/components/tabs';
import type { MDXComponents } from 'mdx/types';
import { Mermaid } from './components/mermaid';
import IOU from './components/iou';
import {
  EditableCodeBlock,
  EditableValue,
  EditableForm,
  EditableInput,
} from './components/editable-code-block';

// use this function to get MDX components, you will need it for rendering MDX
export function getMDXComponents(components?: MDXComponents): MDXComponents {
  return {
    ...defaultMdxComponents,
    Mermaid,
    IOU,
    EditableCodeBlock,
    EditableValue,
    EditableForm,
    EditableInput,
    ...TabsComponents,
    ...components,
  };
}
