import {
  createContext,
  type ReactNode,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react"
import {
  type Locale,
  supportedLocales,
  translations,
  type TranslationKey,
} from "./translations"

const STORAGE_KEY = "cua.locale"

type I18nContextValue = {
  locale: Locale
  setLocale: (locale: Locale) => void
  t: (key: TranslationKey) => string
  formatDateTime: (value: string | number | Date) => string
}

const I18nContext = createContext<I18nContextValue | null>(null)

function isLocale(value: string | null): value is Locale {
  return supportedLocales.includes(value as Locale)
}

function initialLocale(): Locale {
  try {
    const storedLocale = window.localStorage.getItem(STORAGE_KEY)
    if (isLocale(storedLocale)) return storedLocale
  } catch {
    // Storage can be unavailable in privacy-restricted browser contexts.
  }

  const browserLocale = navigator.languages
    .map(language => language.toLowerCase().split("-")[0])
    .find(isLocale)
  return browserLocale ?? "en"
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocale] = useState<Locale>(initialLocale)

  useEffect(() => {
    document.documentElement.lang = locale
    try {
      window.localStorage.setItem(STORAGE_KEY, locale)
    } catch {
      // Keep the in-memory preference when persistent storage is unavailable.
    }
  }, [locale])

  const value = useMemo<I18nContextValue>(() => ({
    locale,
    setLocale,
    t: key => translations[locale][key],
    formatDateTime: value => new Intl.DateTimeFormat(locale, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(new Date(value)),
  }), [locale])

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>
}

export function useI18n(): I18nContextValue {
  const context = useContext(I18nContext)
  if (!context) throw new Error("useI18n must be used within I18nProvider")
  return context
}
