export function Hero({ children }: { children: React.ReactNode }) {
  return (
    <div className="cds-hero-shell not-prose relative mb-12 p-8 md:p-12 lg:p-16">
      <div className="cds-hero-grid pointer-events-none" />
      <div className="pointer-events-none absolute inset-0">
        <div className="absolute -left-20 top-12 h-64 w-64 rounded-full bg-[rgba(97,188,255,0.16)] blur-3xl" />
        <div className="absolute -right-16 top-0 h-72 w-72 rounded-full bg-[rgba(88,200,190,0.12)] blur-3xl" />
        <div className="absolute bottom-0 left-1/3 h-64 w-64 rounded-full bg-[rgba(45,132,198,0.14)] blur-3xl" />
      </div>

      <div className="relative z-10">{children}</div>
    </div>
  );
}
