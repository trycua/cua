export interface UsagePricing {
  vcpuHourUSD: number
  memoryGiBHourUSD: number
}

export const DEFAULT_USAGE_PRICING: UsagePricing = {
  vcpuHourUSD: 0.044625,
  memoryGiBHourUSD: 0.0223125,
}

export function reservedResourceCostUSD(
  cpuCoreHours: number,
  memoryGiBHours: number,
  pricing: UsagePricing = DEFAULT_USAGE_PRICING,
) {
  return cpuCoreHours * pricing.vcpuHourUSD + memoryGiBHours * pricing.memoryGiBHourUSD
}
