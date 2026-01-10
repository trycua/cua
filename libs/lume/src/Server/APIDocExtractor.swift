import Foundation

// MARK: - API Documentation Types

/// Represents documentation for a single API endpoint
struct APIEndpointDoc: Codable {
    let method: String
    let path: String
    let description: String
    let category: String
    let pathParameters: [APIParameterDoc]
    let queryParameters: [APIParameterDoc]
    let requestBody: APIRequestBodyDoc?
    let responseBody: APIResponseDoc
    let statusCodes: [APIStatusCodeDoc]
}

/// Represents documentation for a parameter (path or query)
struct APIParameterDoc: Codable {
    let name: String
    let type: String
    let required: Bool
    let description: String
}

/// Represents documentation for a request body
struct APIRequestBodyDoc: Codable {
    let contentType: String
    let description: String
    let fields: [APIFieldDoc]
}

/// Represents documentation for a response body
struct APIResponseDoc: Codable {
    let contentType: String
    let description: String
    let fields: [APIFieldDoc]?
}

/// Represents documentation for a field in request/response body
struct APIFieldDoc: Codable {
    let name: String
    let type: String
    let required: Bool
    let description: String
    let defaultValue: String?
}

/// Represents documentation for an HTTP status code
struct APIStatusCodeDoc: Codable {
    let code: Int
    let description: String
}

/// Root documentation structure for HTTP API
struct HTTPAPIDocumentation: Codable {
    let basePath: String
    let version: String
    let description: String
    let endpoints: [APIEndpointDoc]
}

// MARK: - API Documentation Extractor

enum APIDocExtractor {
    /// Extract documentation for all API endpoints
    static func extractAll() -> HTTPAPIDocumentation {
        return HTTPAPIDocumentation(
            basePath: "/lume",
            version: Lume.Version.current,
            description: "HTTP API for managing macOS and Linux virtual machines",
            endpoints: allEndpoints
        )
    }

    /// All documented API endpoints
    private static var allEndpoints: [APIEndpointDoc] {
        return [
            // VM Management
            listVMs,
            getVM,
            createVM,
            deleteVM,
            cloneVM,
            updateVM,
            runVM,
            stopVM,

            // Image Management
            getImages,
            getIPSW,
            pullImage,
            pushImage,
            pruneImages,

            // Configuration
            getConfig,
            updateConfig,
            getLocations,
            addLocation,
            removeLocation,
            setDefaultLocation,

            // Logs
            getLogs,
        ]
    }

    // MARK: - VM Management Endpoints

    private static var listVMs: APIEndpointDoc {
        APIEndpointDoc(
            method: "GET",
            path: "/lume/vms",
            description: "List all virtual machines",
            category: "VM Management",
            pathParameters: [],
            queryParameters: [
                APIParameterDoc(name: "storage", type: "string", required: false, description: "Filter by storage location name")
            ],
            requestBody: nil,
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Array of VM details objects",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "Success"),
                APIStatusCodeDoc(code: 400, description: "Bad request")
            ]
        )
    }

    private static var getVM: APIEndpointDoc {
        APIEndpointDoc(
            method: "GET",
            path: "/lume/vms/:name",
            description: "Get detailed information about a specific virtual machine",
            category: "VM Management",
            pathParameters: [
                APIParameterDoc(name: "name", type: "string", required: true, description: "Name of the VM")
            ],
            queryParameters: [
                APIParameterDoc(name: "storage", type: "string", required: false, description: "VM storage location to use")
            ],
            requestBody: nil,
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "VM details object with name, os, cpuCount, memorySize, diskSize, display, status, vncUrl, ipAddress, locationName",
                fields: [
                    APIFieldDoc(name: "name", type: "string", required: true, description: "VM name", defaultValue: nil),
                    APIFieldDoc(name: "os", type: "string", required: true, description: "Operating system (macOS or linux)", defaultValue: nil),
                    APIFieldDoc(name: "cpuCount", type: "integer", required: true, description: "Number of CPU cores", defaultValue: nil),
                    APIFieldDoc(name: "memorySize", type: "integer", required: true, description: "Memory size in bytes", defaultValue: nil),
                    APIFieldDoc(name: "diskSize", type: "object", required: true, description: "Disk size with allocated and total", defaultValue: nil),
                    APIFieldDoc(name: "display", type: "string", required: true, description: "Display resolution (WIDTHxHEIGHT)", defaultValue: nil),
                    APIFieldDoc(name: "status", type: "string", required: true, description: "VM status (running, stopped)", defaultValue: nil),
                    APIFieldDoc(name: "vncUrl", type: "string", required: false, description: "VNC URL if running", defaultValue: nil),
                    APIFieldDoc(name: "ipAddress", type: "string", required: false, description: "IP address if running", defaultValue: nil),
                    APIFieldDoc(name: "locationName", type: "string", required: true, description: "Storage location name", defaultValue: nil)
                ]
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "Success"),
                APIStatusCodeDoc(code: 400, description: "VM not found or invalid request")
            ]
        )
    }

    private static var createVM: APIEndpointDoc {
        APIEndpointDoc(
            method: "POST",
            path: "/lume/vms",
            description: "Create a new virtual machine",
            category: "VM Management",
            pathParameters: [],
            queryParameters: [],
            requestBody: APIRequestBodyDoc(
                contentType: "application/json",
                description: "VM creation parameters",
                fields: [
                    APIFieldDoc(name: "name", type: "string", required: true, description: "Name for the virtual machine", defaultValue: nil),
                    APIFieldDoc(name: "os", type: "string", required: true, description: "Operating system to install (macOS or linux)", defaultValue: nil),
                    APIFieldDoc(name: "cpu", type: "integer", required: true, description: "Number of CPU cores", defaultValue: nil),
                    APIFieldDoc(name: "memory", type: "string", required: true, description: "Memory size (e.g., 8GB)", defaultValue: nil),
                    APIFieldDoc(name: "diskSize", type: "string", required: true, description: "Disk size (e.g., 50GB)", defaultValue: nil),
                    APIFieldDoc(name: "display", type: "string", required: true, description: "Display resolution (e.g., 1024x768)", defaultValue: nil),
                    APIFieldDoc(name: "ipsw", type: "string", required: false, description: "Path to IPSW file or 'latest' for macOS VMs", defaultValue: nil),
                    APIFieldDoc(name: "storage", type: "string", required: false, description: "VM storage location to use", defaultValue: nil)
                ]
            ),
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Success message with VM name",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "VM created successfully"),
                APIStatusCodeDoc(code: 400, description: "Invalid request body or VM creation failed")
            ]
        )
    }

    private static var deleteVM: APIEndpointDoc {
        APIEndpointDoc(
            method: "DELETE",
            path: "/lume/vms/:name",
            description: "Delete a virtual machine and its associated files",
            category: "VM Management",
            pathParameters: [
                APIParameterDoc(name: "name", type: "string", required: true, description: "Name of the VM to delete")
            ],
            queryParameters: [
                APIParameterDoc(name: "storage", type: "string", required: false, description: "VM storage location")
            ],
            requestBody: nil,
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Empty response on success",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "VM deleted successfully"),
                APIStatusCodeDoc(code: 400, description: "VM not found or deletion failed")
            ]
        )
    }

    private static var cloneVM: APIEndpointDoc {
        APIEndpointDoc(
            method: "POST",
            path: "/lume/vms/clone",
            description: "Create a copy of an existing virtual machine",
            category: "VM Management",
            pathParameters: [],
            queryParameters: [],
            requestBody: APIRequestBodyDoc(
                contentType: "application/json",
                description: "Clone parameters",
                fields: [
                    APIFieldDoc(name: "name", type: "string", required: true, description: "Name of the source VM", defaultValue: nil),
                    APIFieldDoc(name: "newName", type: "string", required: true, description: "Name for the cloned VM", defaultValue: nil),
                    APIFieldDoc(name: "sourceLocation", type: "string", required: false, description: "Source VM storage location", defaultValue: nil),
                    APIFieldDoc(name: "destLocation", type: "string", required: false, description: "Destination VM storage location", defaultValue: nil)
                ]
            ),
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Success message with source and destination names",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "VM cloned successfully"),
                APIStatusCodeDoc(code: 400, description: "Clone operation failed")
            ]
        )
    }

    private static var updateVM: APIEndpointDoc {
        APIEndpointDoc(
            method: "PATCH",
            path: "/lume/vms/:name",
            description: "Update virtual machine configuration settings",
            category: "VM Management",
            pathParameters: [
                APIParameterDoc(name: "name", type: "string", required: true, description: "Name of the VM to update")
            ],
            queryParameters: [],
            requestBody: APIRequestBodyDoc(
                contentType: "application/json",
                description: "Settings to update (all fields optional)",
                fields: [
                    APIFieldDoc(name: "cpu", type: "integer", required: false, description: "New number of CPU cores", defaultValue: nil),
                    APIFieldDoc(name: "memory", type: "string", required: false, description: "New memory size (e.g., 16GB)", defaultValue: nil),
                    APIFieldDoc(name: "diskSize", type: "string", required: false, description: "New disk size (e.g., 100GB)", defaultValue: nil),
                    APIFieldDoc(name: "display", type: "string", required: false, description: "New display resolution", defaultValue: nil),
                    APIFieldDoc(name: "storage", type: "string", required: false, description: "VM storage location", defaultValue: nil)
                ]
            ),
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Success message",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "Settings updated successfully"),
                APIStatusCodeDoc(code: 400, description: "Invalid settings or update failed")
            ]
        )
    }

    private static var runVM: APIEndpointDoc {
        APIEndpointDoc(
            method: "POST",
            path: "/lume/vms/:name/run",
            description: "Start a virtual machine",
            category: "VM Management",
            pathParameters: [
                APIParameterDoc(name: "name", type: "string", required: true, description: "Name of the VM to start")
            ],
            queryParameters: [],
            requestBody: APIRequestBodyDoc(
                contentType: "application/json",
                description: "Run options (all fields optional)",
                fields: [
                    APIFieldDoc(name: "noDisplay", type: "boolean", required: false, description: "Run without VNC display", defaultValue: "false"),
                    APIFieldDoc(name: "sharedDirectories", type: "array", required: false, description: "Directories to share with the VM", defaultValue: nil),
                    APIFieldDoc(name: "recoveryMode", type: "boolean", required: false, description: "Boot macOS VM in recovery mode", defaultValue: "false"),
                    APIFieldDoc(name: "storage", type: "string", required: false, description: "VM storage location", defaultValue: nil)
                ]
            ),
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Message indicating VM start was initiated",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 202, description: "VM start initiated (async operation)"),
                APIStatusCodeDoc(code: 400, description: "Invalid request or VM not found")
            ]
        )
    }

    private static var stopVM: APIEndpointDoc {
        APIEndpointDoc(
            method: "POST",
            path: "/lume/vms/:name/stop",
            description: "Stop a running virtual machine",
            category: "VM Management",
            pathParameters: [
                APIParameterDoc(name: "name", type: "string", required: true, description: "Name of the VM to stop")
            ],
            queryParameters: [],
            requestBody: APIRequestBodyDoc(
                contentType: "application/json",
                description: "Stop options (optional)",
                fields: [
                    APIFieldDoc(name: "storage", type: "string", required: false, description: "VM storage location", defaultValue: nil)
                ]
            ),
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Success message",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "VM stopped successfully"),
                APIStatusCodeDoc(code: 400, description: "Stop operation failed")
            ]
        )
    }

    // MARK: - Image Management Endpoints

    private static var getImages: APIEndpointDoc {
        APIEndpointDoc(
            method: "GET",
            path: "/lume/images",
            description: "List available images from local cache",
            category: "Image Management",
            pathParameters: [],
            queryParameters: [
                APIParameterDoc(name: "organization", type: "string", required: false, description: "Organization to list images for (default: trycua)")
            ],
            requestBody: nil,
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Array of image objects with repository and imageId",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "Success"),
                APIStatusCodeDoc(code: 400, description: "Failed to list images")
            ]
        )
    }

    private static var getIPSW: APIEndpointDoc {
        APIEndpointDoc(
            method: "GET",
            path: "/lume/ipsw",
            description: "Get the latest macOS restore image (IPSW) URL",
            category: "Image Management",
            pathParameters: [],
            queryParameters: [],
            requestBody: nil,
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Object with url field containing the IPSW download URL",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "Success"),
                APIStatusCodeDoc(code: 400, description: "Failed to get IPSW URL")
            ]
        )
    }

    private static var pullImage: APIEndpointDoc {
        APIEndpointDoc(
            method: "POST",
            path: "/lume/pull",
            description: "Pull a VM image from a container registry",
            category: "Image Management",
            pathParameters: [],
            queryParameters: [],
            requestBody: APIRequestBodyDoc(
                contentType: "application/json",
                description: "Pull parameters",
                fields: [
                    APIFieldDoc(name: "image", type: "string", required: true, description: "Image to pull (format: name:tag)", defaultValue: nil),
                    APIFieldDoc(name: "name", type: "string", required: false, description: "Name for the resulting VM", defaultValue: nil),
                    APIFieldDoc(name: "registry", type: "string", required: false, description: "Container registry URL", defaultValue: "ghcr.io"),
                    APIFieldDoc(name: "organization", type: "string", required: false, description: "Organization to pull from", defaultValue: "trycua"),
                    APIFieldDoc(name: "storage", type: "string", required: false, description: "VM storage location", defaultValue: nil)
                ]
            ),
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Success message with image and name",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "Image pulled successfully"),
                APIStatusCodeDoc(code: 400, description: "Pull operation failed")
            ]
        )
    }

    private static var pushImage: APIEndpointDoc {
        APIEndpointDoc(
            method: "POST",
            path: "/lume/vms/push",
            description: "Push a VM image to a container registry",
            category: "Image Management",
            pathParameters: [],
            queryParameters: [],
            requestBody: APIRequestBodyDoc(
                contentType: "application/json",
                description: "Push parameters",
                fields: [
                    APIFieldDoc(name: "name", type: "string", required: true, description: "Name of the local VM to push", defaultValue: nil),
                    APIFieldDoc(name: "imageName", type: "string", required: true, description: "Base name for the image in the registry", defaultValue: nil),
                    APIFieldDoc(name: "tags", type: "array", required: true, description: "List of tags to push", defaultValue: nil),
                    APIFieldDoc(name: "registry", type: "string", required: false, description: "Container registry URL", defaultValue: "ghcr.io"),
                    APIFieldDoc(name: "organization", type: "string", required: false, description: "Organization to push to", defaultValue: "trycua"),
                    APIFieldDoc(name: "storage", type: "string", required: false, description: "VM storage location", defaultValue: nil),
                    APIFieldDoc(name: "chunkSizeMb", type: "integer", required: false, description: "Chunk size for upload in MB", defaultValue: "512")
                ]
            ),
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Message indicating push was initiated",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 202, description: "Push initiated (async operation)"),
                APIStatusCodeDoc(code: 400, description: "Invalid request")
            ]
        )
    }

    private static var pruneImages: APIEndpointDoc {
        APIEndpointDoc(
            method: "POST",
            path: "/lume/prune",
            description: "Remove cached images to free up disk space",
            category: "Image Management",
            pathParameters: [],
            queryParameters: [],
            requestBody: nil,
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Success message",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "Images pruned successfully"),
                APIStatusCodeDoc(code: 400, description: "Prune operation failed")
            ]
        )
    }

    // MARK: - Configuration Endpoints

    private static var getConfig: APIEndpointDoc {
        APIEndpointDoc(
            method: "GET",
            path: "/lume/config",
            description: "Get current Lume configuration settings",
            category: "Configuration",
            pathParameters: [],
            queryParameters: [],
            requestBody: nil,
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Configuration object with vmLocations, defaultLocationName, cacheDirectory, cachingEnabled",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "Success"),
                APIStatusCodeDoc(code: 400, description: "Failed to get config")
            ]
        )
    }

    private static var updateConfig: APIEndpointDoc {
        APIEndpointDoc(
            method: "POST",
            path: "/lume/config",
            description: "Update Lume configuration settings",
            category: "Configuration",
            pathParameters: [],
            queryParameters: [],
            requestBody: APIRequestBodyDoc(
                contentType: "application/json",
                description: "Configuration fields to update (all optional)",
                fields: [
                    APIFieldDoc(name: "homeDirectory", type: "string", required: false, description: "VM home directory path", defaultValue: nil),
                    APIFieldDoc(name: "cacheDirectory", type: "string", required: false, description: "Cache directory path", defaultValue: nil),
                    APIFieldDoc(name: "cachingEnabled", type: "boolean", required: false, description: "Enable or disable image caching", defaultValue: nil)
                ]
            ),
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Success message",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "Configuration updated successfully"),
                APIStatusCodeDoc(code: 400, description: "Invalid request")
            ]
        )
    }

    private static var getLocations: APIEndpointDoc {
        APIEndpointDoc(
            method: "GET",
            path: "/lume/config/locations",
            description: "List all VM storage locations",
            category: "Configuration",
            pathParameters: [],
            queryParameters: [],
            requestBody: nil,
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Array of location objects with name and path",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "Success"),
                APIStatusCodeDoc(code: 400, description: "Failed to get locations")
            ]
        )
    }

    private static var addLocation: APIEndpointDoc {
        APIEndpointDoc(
            method: "POST",
            path: "/lume/config/locations",
            description: "Add a new VM storage location",
            category: "Configuration",
            pathParameters: [],
            queryParameters: [],
            requestBody: APIRequestBodyDoc(
                contentType: "application/json",
                description: "Location details",
                fields: [
                    APIFieldDoc(name: "name", type: "string", required: true, description: "Storage location name", defaultValue: nil),
                    APIFieldDoc(name: "path", type: "string", required: true, description: "Path to storage directory", defaultValue: nil)
                ]
            ),
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Success message with name and path",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "Location added successfully"),
                APIStatusCodeDoc(code: 400, description: "Invalid request or location already exists")
            ]
        )
    }

    private static var removeLocation: APIEndpointDoc {
        APIEndpointDoc(
            method: "DELETE",
            path: "/lume/config/locations/:name",
            description: "Remove a VM storage location",
            category: "Configuration",
            pathParameters: [
                APIParameterDoc(name: "name", type: "string", required: true, description: "Name of the location to remove")
            ],
            queryParameters: [],
            requestBody: nil,
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Success message",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "Location removed successfully"),
                APIStatusCodeDoc(code: 400, description: "Location not found or cannot be removed")
            ]
        )
    }

    private static var setDefaultLocation: APIEndpointDoc {
        APIEndpointDoc(
            method: "POST",
            path: "/lume/config/locations/default/:name",
            description: "Set the default VM storage location",
            category: "Configuration",
            pathParameters: [
                APIParameterDoc(name: "name", type: "string", required: true, description: "Name of the location to set as default")
            ],
            queryParameters: [],
            requestBody: nil,
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Success message",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "Default location set successfully"),
                APIStatusCodeDoc(code: 400, description: "Location not found")
            ]
        )
    }

    // MARK: - Logs Endpoint

    private static var getLogs: APIEndpointDoc {
        APIEndpointDoc(
            method: "GET",
            path: "/lume/logs",
            description: "Retrieve Lume server logs",
            category: "Logs",
            pathParameters: [],
            queryParameters: [
                APIParameterDoc(name: "type", type: "string", required: false, description: "Log type: 'info', 'error', or 'all' (default: all)"),
                APIParameterDoc(name: "lines", type: "integer", required: false, description: "Number of lines to return from end of log")
            ],
            requestBody: nil,
            responseBody: APIResponseDoc(
                contentType: "application/json",
                description: "Object with 'info' and/or 'error' fields containing log content",
                fields: nil
            ),
            statusCodes: [
                APIStatusCodeDoc(code: 200, description: "Success"),
                APIStatusCodeDoc(code: 400, description: "Failed to read logs")
            ]
        )
    }
}
