// Example prompts component
// Adapted from cloud/src/website/app/components/playground/ExamplePrompts.tsx

import { motion } from 'motion/react';
import { Tooltip, TooltipContent, TooltipTrigger } from '../ui/tooltip';

export interface ExamplePrompt {
  id: string;
  title: string;
  description: string;
  icon: string; // Path to icon/logo
  prompt: string;
}

export const EXAMPLE_PROMPTS: ExamplePrompt[] = [
  {
    id: 'job-application',
    title: 'Job Application',
    description: 'Apply to jobs on Ashby',
    icon: '/img/logos/ashby.png',
    prompt: `Help me apply to a job. Here's what I need you to do:

1. Take a screenshot and assess the current screen state - look at what's currently visible.
2. Open Firefox and visit https://www.overleaf.com/latex/templates/jakes-resume/syzfjbzwjncs.pdf
3. Download the PDF by clicking the download button in the top right corner of the PDF viewer
4. Once downloaded, navigate to https://www.ashbyhq.com/job_board_example_html/application-form-only.html?ashby_jid=dd883672-4381-4aed-a28d-09a2af002772
5. Click on "First Posting" to open the job application form
6. Fill out the application form with the following details:
   - First Name: Jake
   - Last Name: Ryan
   - Email: jake@su.edu
   - Phone: 123-456-7890
7. Upload the resume PDF you downloaded earlier by clicking the resume upload field
8. Complete any remaining required fields in the form
9. Review all entered information to make sure it's correct
10. Submit the application by clicking the submit button`,
  },
  {
    id: 'yc-companies',
    title: 'Browse YC Companies',
    description: 'Explore latest Y Combinator startups',
    icon: '/img/logos/yc.svg',
    prompt: `Help me research the latest Y Combinator companies. Here's what I need you to do:

1. Take a screenshot and assess the current screen state - look at what's currently visible.
2. Open Firefox and visit https://www.ycombinator.com/companies
3. Click on the "Batch" filter dropdown and select the most recent batch (e.g., W25 or S24)
4. Browse through the list of companies in this batch
5. For each of the first 5 companies you see:
   - Click on the company to view their details page
   - Note down the company name
   - Note down what they do (their one-line description)
   - Note down the names of the founders listed
   - Navigate back to the companies list
6. Open a text editor application
7. Create a CSV file with the following columns: Company Name, Description, Founders
8. Enter the information for all 5 companies you researched
9. Save the file as "yc_companies.csv" on the desktop`,
  },
  {
    id: 'install-claude-code',
    title: 'Install Claude Code',
    description: 'Set up Claude Code on this machine',
    icon: '/img/logos/claude-code.png',
    prompt: `Help me install Claude Code on this machine. Here's what I need you to do:

1. Take a screenshot and assess the current screen state - look at what's currently visible.
2. Open Firefox and visit https://docs.anthropic.com/en/docs/claude-code/overview
3. Look for the installation instructions on the page
4. Open the Terminal application
5. Check if Node.js is installed by running: node --version
6. If Node.js is not installed (command not found), visit https://nodejs.org and download the LTS version installer, then run the installer
7. Once Node.js is confirmed installed, install Claude Code globally by running: npm install -g @anthropic-ai/claude-code
8. Wait for the installation to complete
9. Verify the installation was successful by running: claude --version
10. The output should show the Claude Code version number, confirming it's installed correctly`,
  },
];

interface ExamplePromptsProps {
  /** Callback when a prompt is selected */
  onSelectPrompt: (prompt: string) => void;
  /** Optional telemetry callback when example prompt is selected */
  onExamplePromptSelected?: (promptId: string, promptTitle: string) => void;
  /** Custom prompts to display (defaults to EXAMPLE_PROMPTS) */
  prompts?: ExamplePrompt[];
}

export function ExamplePrompts({
  onSelectPrompt,
  onExamplePromptSelected,
  prompts = EXAMPLE_PROMPTS,
}: ExamplePromptsProps) {
  const handleSelectPrompt = (example: ExamplePrompt) => {
    onExamplePromptSelected?.(example.id, example.title);
    onSelectPrompt(example.prompt);
  };

  return (
    <motion.div
      className="mt-6 flex w-full flex-col items-center gap-2"
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3, delay: 0.2 }}
    >
      <div className="flex flex-wrap justify-center gap-2">
        {prompts.map((example, index) => (
          <Tooltip key={example.id}>
            <TooltipTrigger asChild>
              <motion.button
                type="button"
                onClick={() => handleSelectPrompt(example)}
                className="group flex items-center gap-2 rounded-full border border-neutral-200 bg-white px-3 py-1.5 text-left transition-all hover:border-neutral-300 hover:bg-neutral-50 hover:shadow-sm dark:border-neutral-700 dark:bg-neutral-800 dark:hover:border-neutral-600 dark:hover:bg-neutral-700"
                initial={{ opacity: 0, y: 10 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ duration: 0.2, delay: 0.25 + index * 0.05 }}
                whileHover={{ scale: 1.02 }}
                whileTap={{ scale: 0.98 }}
              >
                <img
                  src={example.icon}
                  alt=""
                  className="h-4 w-4 object-contain"
                  onError={(e) => {
                    e.currentTarget.style.display = 'none';
                  }}
                />
                <span className="font-medium text-neutral-700 text-sm dark:text-neutral-200">
                  {example.title}
                </span>
              </motion.button>
            </TooltipTrigger>
            <TooltipContent sideOffset={8}>{example.description}</TooltipContent>
          </Tooltip>
        ))}
      </div>
      <p className="mt-1 text-neutral-400 text-xs dark:text-neutral-500">
        More example prompts coming soon...
      </p>
    </motion.div>
  );
}
