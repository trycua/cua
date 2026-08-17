import type { ReactNode } from "react"
import Box from "@cloudscape-design/components/box"
import SpaceBetween from "@cloudscape-design/components/space-between"

interface PageStateProps {
  title: ReactNode
  children?: ReactNode
  action?: ReactNode
  role?: "status" | "alert"
}

function PageState({ title, children, action, role = "status" }: PageStateProps) {
  return (
    <Box textAlign="center" padding="m">
      <div role={role}>
        <SpaceBetween size="xs">
          <Box variant="h3">{title}</Box>
          {children ? (
            <Box variant="p" color="text-body-secondary">
              {children}
            </Box>
          ) : null}
          {action}
        </SpaceBetween>
      </div>
    </Box>
  )
}

export type PageEmptyProps = Omit<PageStateProps, "role">

export function PageEmpty(props: PageEmptyProps) {
  return <PageState {...props} role="status" />
}

export type PageErrorProps = Omit<PageStateProps, "role">

export function PageError(props: PageErrorProps) {
  return <PageState {...props} role="alert" />
}
