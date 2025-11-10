export function Hero({ children }: { children: React.ReactNode }) {
  return (
    <div className="not-prose relative mb-12 overflow-hidden rounded-xl border border-fd-border bg-gradient-to-br from-fd-background via-fd-muted/30 to-fd-muted/50 p-8 shadow-lg md:p-12 lg:p-16">
      {/* Background Pattern */}
      <div className="pointer-events-none absolute inset-0">
        {/* Grid */}
        <svg
          className="absolute h-full w-full text-fd-foreground"
          xmlns="http://www.w3.org/2000/svg"
        >
          <defs>
            <pattern
              id="hero-grid"
              width="40"
              height="40"
              patternUnits="userSpaceOnUse"
            >
              <path
                d="M 40 0 L 0 0 0 40"
                fill="none"
                stroke="currentColor"
                strokeWidth="0.5"
                opacity="0.1"
              />
            </pattern>
          </defs>
          <rect width="100%" height="100%" fill="url(#hero-grid)" />
        </svg>

        {/* Subtle glow effects */}
        <div className="absolute -right-20 -top-20 h-96 w-96 rounded-full bg-fd-primary/5 blur-3xl" />
        <div className="absolute -bottom-32 -left-20 h-96 w-96 rounded-full bg-fd-primary/5 blur-3xl" />
      </div>

      {/* Content */}
      <div className="relative z-10">
        {children}
      </div>
    </div>
  );
}
