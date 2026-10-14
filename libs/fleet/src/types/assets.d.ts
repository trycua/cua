/// <reference types="vite/client" />

declare module "*.svg" {
  const url: string
  export default url
}

declare module "*.woff2" {
  const url: string
  export default url
}
