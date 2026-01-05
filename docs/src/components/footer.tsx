export function Footer() {
  return (
    <footer className="mt-auto border-t border-fd-border py-6">
      <div className="container mx-auto px-4">
        <div className="flex flex-col md:flex-row justify-between items-center gap-4">
          <p className="text-sm text-fd-muted-foreground">
            © {new Date().getFullYear()} Cua AI, Inc.
          </p>
          <div className="flex gap-4">
            <a
              href="https://cua.ai/privacy-policy"
              className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
            >
              Privacy
            </a>
            <a
              href="https://cua.ai/cookie-policy"
              className="text-sm text-fd-muted-foreground hover:text-fd-foreground transition-colors"
            >
              Cookies
            </a>
          </div>
        </div>
      </div>
    </footer>
  );
}
