export function Footer() {
  return (
    <footer className="mt-auto border-t border-fd-border py-8">
      <div className="container mx-auto px-4">
        <div className="grid grid-cols-1 md:grid-cols-4 gap-8 mb-6">
          {/* Product Links */}
          <div>
            <h3 className="font-semibold text-sm mb-3 text-fd-foreground">Product</h3>
            <ul className="space-y-2">
              <li>
                <a
                  href="https://cua.ai"
                  className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
                >
                  Home
                </a>
              </li>
              <li>
                <a
                  href="https://cua.ai/pricing"
                  className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
                >
                  Pricing
                </a>
              </li>
              <li>
                <a
                  href="https://cua.ai/#features"
                  className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
                >
                  Features
                </a>
              </li>
            </ul>
          </div>

          {/* Documentation Links */}
          <div>
            <h3 className="font-semibold text-sm mb-3 text-fd-foreground">Documentation</h3>
            <ul className="space-y-2">
              <li>
                <a
                  href="/docs"
                  className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
                >
                  Getting Started
                </a>
              </li>
              <li>
                <a
                  href="/docs/agent-sdk/agent-loops"
                  className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
                >
                  Agent Loops
                </a>
              </li>
              <li>
                <a
                  href="/docs/get-started/quickstart"
                  className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
                >
                  Quick Start
                </a>
              </li>
            </ul>
          </div>

          {/* Resources Links */}
          <div>
            <h3 className="font-semibold text-sm mb-3 text-fd-foreground">Resources</h3>
            <ul className="space-y-2">
              <li>
                <a
                  href="https://cua.ai/blog"
                  className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
                >
                  Blog
                </a>
              </li>
              <li>
                <a
                  href="https://github.com/trycua/cua"
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
                >
                  GitHub
                </a>
              </li>
              <li>
                <a
                  href="https://discord.com/invite/mVnXXpdE85"
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
                >
                  Discord Community
                </a>
              </li>
            </ul>
          </div>

          {/* Company Links */}
          <div>
            <h3 className="font-semibold text-sm mb-3 text-fd-foreground">Company</h3>
            <ul className="space-y-2">
              <li>
                <a
                  href="https://cua.ai/about"
                  className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
                >
                  About
                </a>
              </li>
              <li>
                <a
                  href="mailto:hello@trycua.com"
                  className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
                >
                  Contact
                </a>
              </li>
              <li>
                <a
                  href="https://cua.ai/cookie-policy"
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
                >
                  Cookie Policy
                </a>
              </li>
            </ul>
          </div>
        </div>

        {/* Bottom Bar */}
        <div className="pt-6 border-t border-fd-border flex flex-col md:flex-row justify-between items-center gap-4">
          <p className="text-sm text-fd-muted-foreground">
            © {new Date().getFullYear()} Cua. All rights reserved.
          </p>
          <div className="flex gap-4">
            <a
              href="https://cua.ai/privacy"
              className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
            >
              Privacy Policy
            </a>
            <a
              href="https://cua.ai/terms"
              className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
            >
              Terms of Service
            </a>
          </div>
        </div>
      </div>
    </footer>
  );
}
