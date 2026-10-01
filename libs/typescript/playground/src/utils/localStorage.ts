// Generic localStorage utility that works with any object type
export interface StorageConfig<T> {
  storageKey: string;
  serialize?: (item: T) => unknown;
  deserialize?: (item: unknown) => T;
}

// Type guard to check if a value is a record (plain object)
const isRecord = (value: unknown): value is Record<string, unknown> => {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
};

// Type guard to check if a string looks like an ISO date
const isISODateString = (value: string): boolean => {
  return /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}/.test(value);
};

// Default serializer for objects with Date properties
const defaultSerializer = (obj: unknown): unknown => {
  if (!obj || typeof obj !== 'object') return obj;

  if (Array.isArray(obj)) {
    return obj.map(defaultSerializer);
  }

  if (isRecord(obj)) {
    const serialized: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(obj)) {
      if (value instanceof Date) {
        serialized[key] = value.toISOString();
      } else if (value && typeof value === 'object') {
        serialized[key] = defaultSerializer(value);
      } else {
        serialized[key] = value;
      }
    }
    return serialized;
  }

  return obj;
};

// Default deserializer for objects with Date properties
const defaultDeserializer = (obj: unknown): unknown => {
  if (!obj || typeof obj !== 'object') return obj;

  if (Array.isArray(obj)) {
    return obj.map(defaultDeserializer);
  }

  if (isRecord(obj)) {
    const deserialized: Record<string, unknown> = {};

    for (const [key, value] of Object.entries(obj)) {
      if (typeof value === 'string' && isISODateString(value)) {
        // Try to parse as Date if it looks like an ISO string
        const date = new Date(value);
        deserialized[key] = Number.isNaN(date.getTime()) ? value : date;
      } else if (value && typeof value === 'object') {
        deserialized[key] = defaultDeserializer(value);
      } else {
        deserialized[key] = value;
      }
    }

    return deserialized;
  }

  return obj;
};

// Save array of items to localStorage
export const saveItemsToLocalStorage = <T>(items: T[], config: StorageConfig<T>): void => {
  try {
    if (typeof window === 'undefined') return;
    const serializer = config.serialize || defaultSerializer;
    const serializedItems = items.map(serializer);
    localStorage.setItem(config.storageKey, JSON.stringify(serializedItems));
  } catch (error) {
    console.error(`Failed to save items to localStorage (${config.storageKey}):`, error);
  }
};

// Load array of items from localStorage
export const loadItemsFromLocalStorage = <T>(config: StorageConfig<T>): T[] => {
  try {
    if (typeof window === 'undefined') return [];
    const stored = localStorage.getItem(config.storageKey);
    if (!stored) return [];

    const parsedItems: unknown = JSON.parse(stored);

    if (!Array.isArray(parsedItems)) {
      console.warn(`Expected array but got ${typeof parsedItems} from localStorage`);
      return [];
    }

    const deserializer = config.deserialize || defaultDeserializer;
    return parsedItems.map((item) => deserializer(item) as T);
  } catch (error) {
    console.error(`Failed to load items from localStorage (${config.storageKey}):`, error);
    return [];
  }
};

// Save single item to localStorage
export const saveItemToLocalStorage = <T>(item: T, config: StorageConfig<T>): void => {
  try {
    if (typeof window === 'undefined') return;
    const serializer = config.serialize || defaultSerializer;
    const serializedItem = serializer(item);
    localStorage.setItem(config.storageKey, JSON.stringify(serializedItem));
  } catch (error) {
    console.error(`Failed to save item to localStorage (${config.storageKey}):`, error);
  }
};

// Load single item from localStorage
export const loadItemFromLocalStorage = <T>(config: StorageConfig<T>): T | null => {
  try {
    if (typeof window === 'undefined') return null;
    const stored = localStorage.getItem(config.storageKey);
    if (!stored) return null;

    const parsedItem: unknown = JSON.parse(stored);
    const deserializer = config.deserialize || defaultDeserializer;
    return deserializer(parsedItem) as T;
  } catch (error) {
    console.error(`Failed to load item from localStorage (${config.storageKey}):`, error);
    return null;
  }
};

// Clear items from localStorage
export const clearItemsFromLocalStorage = (storageKeys: string[]): void => {
  try {
    if (typeof window === 'undefined') return;
    storageKeys.forEach((key) => {
      localStorage.removeItem(key);
    });
  } catch (error) {
    console.error('Failed to clear items from localStorage:', error);
  }
};
