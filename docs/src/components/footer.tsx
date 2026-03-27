import Link from 'next/link';
import { Mail } from 'lucide-react';

const linkClass = 'text-[var(--ink-muted)] hover:text-[var(--ink-strong)] transition-colors';

export function Footer() {
  return (
    <footer
      className="overflow-hidden rounded-t-2xl px-6 py-16 text-[var(--ink-strong)] md:px-12 lg:px-24"
      style={{
        background: 'var(--bg-elev-1)',
        borderTop: '1px solid var(--line-soft)',
      }}
    >
      <div className="relative mx-auto flex max-w-7xl flex-col gap-16">
        <div className="flex flex-col items-start gap-16 lg:flex-row lg:justify-between">
          {/* Left side - Brand */}
          <div className="flex flex-col items-start gap-8">
            <Link
              href="/"
              className="flex flex-shrink-0 items-center gap-3 font-medium text-4xl text-[var(--ink-strong)]"
            >
              {/* eslint-disable-next-line @next/next/no-img-element */}
              <img
                src="/docs/img/cua-logo-accent.svg"
                alt="Cua"
                className="h-16 w-16"
              />
              <p className="mt-2">
                Cua
              </p>
            </Link>
            <p className="font-medium text-base">
              GIVE EVERY AGENT A CLOUD DESKTOP
            </p>
            <div className="flex items-center gap-2 font-medium text-xs">
              BACKED BY
              <div className="flex items-center gap-1">
                <svg
                  className="h-4 w-4"
                  viewBox="0 0 46.7 47"
                  fill="currentColor"
                >
                  <path
                    d="M0,0v47h46.7V0H0z M31.8,13.5c-2.2,4.3-4.5,8.6-6.8,12.9c-0.1,0.2-0.1,0.3-0.1,0.5c0,3.1,0,6.2,0,9.2
                  c-1.1,0-2.2,0-3.3,0c0-3,0-6,0-9c0-0.3,0-0.7-0.2-1c-2.1-3.9-4.1-7.8-6.2-11.8c-0.5-0.9-1-1.8-1.4-2.8c0.2,0,0.4-0.1,0.6-0.1
                  c1,0,2,0,3,0.1c0.2,0,0.2,0.1,0.3,0.2c1.5,3.1,3,6.3,4.6,9.5c0.3,0.7,0.6,1.3,1,1.9c0.3-0.5,0.6-1,0.9-1.6
                  c1.7-3.4,3.4-6.7,5.1-10.1c1.1,0,2.2-0.1,3.4,0.1C32.5,12.4,32.1,12.9,31.8,13.5z"
                  />
                </svg>
                COMBINATOR
              </div>
            </div>

            {/* Social Icons */}
            <div className="flex items-center gap-8">
              <a
                href="https://linkedin.com/company/cua-ai"
                target="_blank"
                rel="noopener noreferrer"
                className={linkClass}
                aria-label="LinkedIn"
              >
                {/* eslint-disable-next-line @next/next/no-img-element */}
                <img
                  src="/docs/img/logos/linkedin-white.svg"
                  alt="LinkedIn"
                  className="h-4 w-4"
                />
              </a>
              <a
                href="https://twitter.com/trycua"
                target="_blank"
                rel="noopener noreferrer"
                className={linkClass}
                aria-label="X"
              >
                {/* eslint-disable-next-line @next/next/no-img-element */}
                <img src="/docs/img/logos/x-white.svg" alt="X" className="h-4 w-4" />
              </a>
              <a
                href="https://discord.com/invite/cua-ai"
                target="_blank"
                rel="noopener noreferrer"
                className={linkClass}
                aria-label="Discord"
              >
                <svg
                  className="h-4 w-4"
                  fill="currentColor"
                  viewBox="0 0 640 512"
                >
                  <path d="M524.531,69.836a1.5,1.5,0,0,0-.764-.7A485.065,485.065,0,0,0,404.081,32.03a1.816,1.816,0,0,0-1.923.91,337.461,337.461,0,0,0-14.9,30.6,447.848,447.848,0,0,0-134.426,0,309.541,309.541,0,0,0-15.135-30.6,1.89,1.89,0,0,0-1.924-.91A483.689,483.689,0,0,0,116.085,69.137a1.712,1.712,0,0,0-.788.676C39.068,183.651,18.186,294.69,28.43,404.354a2.016,2.016,0,0,0,.765,1.375A487.666,487.666,0,0,0,176.02,479.918a1.9,1.9,0,0,0,2.063-.676A348.2,348.2,0,0,0,208.12,430.4a1.86,1.86,0,0,0-1.019-2.588,321.173,321.173,0,0,1-45.868-21.853,1.885,1.885,0,0,1-.185-3.126c3.082-2.309,6.166-4.711,9.109-7.137a1.819,1.819,0,0,1,1.9-.256c96.229,43.917,200.41,43.917,295.5,0a1.812,1.812,0,0,1,1.924.233c2.944,2.426,6.027,4.851,9.132,7.16a1.884,1.884,0,0,1-.162,3.126,301.407,301.407,0,0,1-45.89,21.83,1.875,1.875,0,0,0-1,2.611,391.055,391.055,0,0,0,30.014,48.815,1.864,1.864,0,0,0,2.063.7A486.048,486.048,0,0,0,610.7,405.729a1.882,1.882,0,0,0,.765-1.352C623.729,277.594,590.933,167.465,524.531,69.836ZM222.491,337.58c-28.972,0-52.844-26.587-52.844-59.239S193.056,219.1,222.491,219.1c29.665,0,53.306,26.82,52.843,59.239C275.334,310.993,251.924,337.58,222.491,337.58Zm195.38,0c-28.971,0-52.843-26.587-52.843-59.239S388.437,219.1,417.871,219.1c29.667,0,53.307,26.82,52.844,59.239C470.715,310.993,447.538,337.58,417.871,337.58Z" />
                </svg>
              </a>
              <a
                href="mailto:team@trycua.com"
                target="_blank"
                rel="noopener noreferrer"
                className={linkClass}
                aria-label="Contact"
              >
                <Mail className="h-4 w-4" />
              </a>
            </div>
          </div>

          {/* Columns wrapper */}
          <div className="flex gap-16">
            {/* Product Column */}
            <div className="flex flex-col gap-4 font-medium text-sm">
              Product
              <a href="https://cua.ai/pricing" className={linkClass}>
                Pricing
              </a>
              <a href="https://app.cua.ai" className={linkClass}>
                Dashboard
              </a>
            </div>

            {/* Resources Column */}
            <div className="flex flex-col gap-4 font-medium text-sm">
              Resources
              <Link href="/cua/guide/get-started/what-is-cua" className={linkClass}>
                Docs
              </Link>
              <a href="https://cua.ai/blog" className={linkClass}>
                Blog
              </a>
            </div>

            {/* Company Column */}
            <div className="flex flex-col gap-4 font-medium text-sm">
              Company
              <a href="https://cua.ai/careers" className={linkClass}>
                Careers
              </a>
              <a href="https://cua.ai/privacy-policy" className={linkClass}>
                Privacy policy
              </a>
              <a href="https://cua.ai/cookie-policy" className={linkClass}>
                Cookie policy
              </a>
            </div>
          </div>
        </div>

        {/* Bottom bar */}
        <div className="flex flex-col gap-4 text-[var(--ink-muted)] text-sm lg:flex-row lg:items-center lg:justify-between">
          <p>&copy; {new Date().getFullYear()} Cua AI, Inc.</p>
          <p>Made with 💖 in San Francisco.</p>
        </div>
      </div>
    </footer>
  );
}
