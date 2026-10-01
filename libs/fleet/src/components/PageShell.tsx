import type { ReactNode } from "react"
import ContentLayout from "@cloudscape-design/components/content-layout"
import Header from "@cloudscape-design/components/header"

export interface PageShellProps {
  eyebrow?: string
  title: ReactNode
  counter?: string
  description?: ReactNode
  actions?: ReactNode
  secondaryActions?: ReactNode
  primaryAction?: ReactNode
  className?: string
  children: ReactNode
}

export function PageShell({
  eyebrow,
  title,
  counter,
  description,
  actions,
  secondaryActions,
  primaryAction,
  className,
  children,
}: PageShellProps) {
  return (
    <ContentLayout
      className={className}
      disableOverlap
      header={
        <div className="cua-pagehead">
          {eyebrow ? <p className="cua-pagehead__eyebrow">{eyebrow}</p> : null}
          <Header
            variant="h1"
            counter={counter}
            description={description}
            actions={(actions || secondaryActions || primaryAction) ? (
              <div className="cua-pagehead__actions">
                {actions ?? (
                  <>
                    {secondaryActions ? (
                      <div className="cua-pagehead__secondary-actions">
                        {secondaryActions}
                      </div>
                    ) : null}
                    {primaryAction ? (
                      <div className="cua-pagehead__primary-action">
                        {primaryAction}
                      </div>
                    ) : null}
                  </>
                )}
              </div>
            ) : undefined}
          >
            <span className="cua-pagehead__title">{title}</span>
          </Header>
        </div>
      }
    >
      <div className="cua-pagebody">{children}</div>
    </ContentLayout>
  )
}
