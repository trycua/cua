import { createContext, useContext } from "react"
import { FlashbarProps } from "@cloudscape-design/components/flashbar"

export type FlashMsg = Omit<
  FlashbarProps.MessageDefinition,
  "id" | "onDismiss" | "dismissible"
>

export const FlashContext = createContext<{
  push: (msg: FlashMsg) => void
}>({ push: () => {} })

export const useFlash = () => useContext(FlashContext)
