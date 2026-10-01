import { forwardRef } from "react"
import Button, {
  type ButtonProps,
} from "@cloudscape-design/components/button"

export type CuaButtonProps = ButtonProps & {
  tone?: "primary" | "secondary" | "icon" | "danger"
}

export const CuaButton = forwardRef<ButtonProps.Ref, CuaButtonProps>(
  function CuaButton({ tone = "secondary", variant, ...props }, ref) {
    return (
      <span className={`cua-dashboard-button cua-dashboard-button--${tone}`}>
        <Button
          {...props}
          ref={ref}
          variant={variant ?? (tone === "primary" ? "primary" : "normal")}
        />
      </span>
    )
  },
)
