/* eslint-disable */
/* tslint:disable */
// @ts-nocheck
/*
 * ---------------------------------------------------------------
 * ## THIS FILE WAS GENERATED VIA SWAGGER-TYPESCRIPT-API        ##
 * ##                                                           ##
 * ## AUTHOR: acacode                                           ##
 * ## SOURCE: https://github.com/acacode/swagger-typescript-api ##
 * ---------------------------------------------------------------
 */

export interface HandlersConfigResponse {
  /**
   * Admin is true when the caller is in input.flags.admin_subs (OPA-evaluated).
   * Non-admins get the customer view: infra-only nav (Nodes, Operator events)
   * is hidden in the SPA and the corresponding kubectl-proxy paths are denied
   * server-side by authz.rego.
   */
  admin?: boolean;
}

export interface HandlersCreateKeyRequest {
  /** @example "ci-prod" */
  name?: string;
  /** @example "test-pool" */
  namespace?: string;
}

export interface HandlersCreateKeyResponse {
  client_id?: string;
  client_secret?: string;
  name?: string;
  namespace?: string;
  token_url?: string;
}

export interface HandlersCreateNamespaceRequest {
  /** @example "my-workspace" */
  name?: string;
}

export interface HandlersCreatePoolTemplateRequest {
  config?: object;
  /** @example "gpu-large" */
  name?: string;
}

export interface HandlersCreateUserKeyRequest {
  /** @example "my-ci-key" */
  name?: string;
  /** @example ["[\"ns1\"","\"ns2\"]"] */
  scope?: string[];
}

export interface HandlersCreateUserKeyResponse {
  client_id?: string;
  client_secret?: string;
  name?: string;
  scope?: string[];
  token_url?: string;
}

export interface HandlersErrorResponse {
  error?: string;
}

export interface HandlersHealthResponse {
  ok?: boolean;
}

export interface HandlersListKeysResponse {
  keys?: KeycloakKeyClient[];
}

export interface HandlersListUserKeysResponse {
  keys?: HandlersUserKeyResponse[];
}

export interface HandlersNamespaceResponse {
  createdAt?: string;
  labels?: Record<string, string>;
  name?: string;
  status?: string;
}

export interface HandlersPoolTemplateResponse {
  config?: object;
  createdAt?: string;
  name?: string;
  user?: string;
}

export interface HandlersUserKeyResponse {
  client_id?: string;
  id?: string;
  name?: string;
  scope?: string[];
}

export interface KeycloakKeyClient {
  client_id?: string;
  id?: string;
  name?: string;
  namespace?: string;
  owner_sub?: string;
}

export type BatchLanesCreateError = Record<string, string>;

export type BatchLanesDeleteError = Record<string, string>;

export type BatchSubmitCreateError = Record<string, string>;

export type BatchDeleteError = Record<string, string>;

export type BatchResultsListError = Record<string, string>;

export type BatchStatusListError = Record<string, string>;

export type ConfigListData = HandlersConfigResponse;

export type ConfigListError = HandlersErrorResponse;

export type GatewayDetailData = string;

export type GatewayDetailError = HandlersErrorResponse;

export type GetK8SData = string;

export type GetK8SError = HandlersErrorResponse;

export type KeysListData = HandlersListKeysResponse;

export type KeysListError = HandlersErrorResponse;

export type KeysCreateData = HandlersCreateKeyResponse;

export type KeysCreateError = HandlersErrorResponse;

export type KeysDeleteData = any;

export type KeysDeleteError = HandlersErrorResponse;

export type LabelDeleteError = Record<string, string>;

export type LabelBatchCreateError = Record<string, string>;

export type LabelResultsListError = Record<string, string>;

export type LabelStatusListError = Record<string, string>;

export type NamespacesListData = HandlersNamespaceResponse[];

export type NamespacesListError = HandlersErrorResponse;

export type NamespacesCreateData = HandlersNamespaceResponse;

export type NamespacesCreateError = HandlersErrorResponse;

export type NamespacesDeleteData = any;

export type NamespacesDeleteError = HandlersErrorResponse;

export type OrchDetailData = string;

export type OrchDetailError = HandlersErrorResponse;

export type PoolTemplatesListData = HandlersPoolTemplateResponse[];

export type PoolTemplatesListError = HandlersErrorResponse;

export type PoolTemplatesCreateData = HandlersPoolTemplateResponse;

export type PoolTemplatesCreateError = HandlersErrorResponse;

export type PoolTemplatesDetailData = HandlersPoolTemplateResponse;

export type PoolTemplatesDetailError = HandlersErrorResponse;

export type PoolTemplatesDeleteData = any;

export type PoolTemplatesDeleteError = HandlersErrorResponse;

export type GetSvcData = string;

export type GetSvcError = HandlersErrorResponse;

export type UserKeysListData = HandlersListUserKeysResponse;

export type UserKeysListError = HandlersErrorResponse;

export type UserKeysCreateData = HandlersCreateUserKeyResponse;

export type UserKeysCreateError = HandlersErrorResponse;

export type UserKeysDeleteData = any;

export type UserKeysDeleteError = HandlersErrorResponse;

export type HealthzListData = HandlersHealthResponse;

export type QueryParamsType = Record<string | number, any>;
export type ResponseFormat = keyof Omit<Body, "body" | "bodyUsed">;

export interface FullRequestParams extends Omit<RequestInit, "body"> {
  /** set parameter to `true` for call `securityWorker` for this request */
  secure?: boolean;
  /** request path */
  path: string;
  /** content type of request body */
  type?: ContentType;
  /** query params */
  query?: QueryParamsType;
  /** format of response (i.e. response.json() -> format: "json") */
  format?: ResponseFormat;
  /** request body */
  body?: unknown;
  /** base url */
  baseUrl?: string;
  /** request cancellation token */
  cancelToken?: CancelToken;
}

export type RequestParams = Omit<
  FullRequestParams,
  "body" | "method" | "query" | "path"
>;

export interface ApiConfig<SecurityDataType = unknown> {
  baseUrl?: string;
  baseApiParams?: Omit<RequestParams, "baseUrl" | "cancelToken" | "signal">;
  securityWorker?: (
    securityData: SecurityDataType | null,
  ) => Promise<RequestParams | void> | RequestParams | void;
  customFetch?: typeof fetch;
}

export interface HttpResponse<D extends unknown, E extends unknown = unknown>
  extends Response {
  data: D;
  error: E;
}

type CancelToken = Symbol | string | number;

export enum ContentType {
  Json = "application/json",
  JsonApi = "application/vnd.api+json",
  FormData = "multipart/form-data",
  UrlEncoded = "application/x-www-form-urlencoded",
  Text = "text/plain",
}

export class HttpClient<SecurityDataType = unknown> {
  public baseUrl: string = "/";
  private securityData: SecurityDataType | null = null;
  private securityWorker?: ApiConfig<SecurityDataType>["securityWorker"];
  private abortControllers = new Map<CancelToken, AbortController>();
  private customFetch: typeof fetch = (...fetchParams) => {
    const runtimeFetch = globalThis.fetch;
    if (typeof runtimeFetch !== "function") {
      throw new Error(
        "Fetch API is unavailable. @trycua/cyclops requires Node.js 18+ or an injected customFetch implementation.",
      );
    }
    return runtimeFetch(...fetchParams);
  };

  private baseApiParams: RequestParams = {
    credentials: "same-origin",
    headers: {},
    redirect: "follow",
    referrerPolicy: "no-referrer",
  };

  constructor(apiConfig: ApiConfig<SecurityDataType> = {}) {
    Object.assign(this, apiConfig);
  }

  public setSecurityData = (data: SecurityDataType | null) => {
    this.securityData = data;
  };

  protected encodeQueryParam(key: string, value: any) {
    const encodedKey = encodeURIComponent(key);
    return `${encodedKey}=${encodeURIComponent(typeof value === "number" ? value : `${value}`)}`;
  }

  protected addQueryParam(query: QueryParamsType, key: string) {
    return this.encodeQueryParam(key, query[key]);
  }

  protected addArrayQueryParam(query: QueryParamsType, key: string) {
    const value = query[key];
    return value.map((v: any) => this.encodeQueryParam(key, v)).join("&");
  }

  protected toQueryString(rawQuery?: QueryParamsType): string {
    const query = rawQuery || {};
    const keys = Object.keys(query).filter(
      (key) => "undefined" !== typeof query[key],
    );
    return keys
      .map((key) =>
        Array.isArray(query[key])
          ? this.addArrayQueryParam(query, key)
          : this.addQueryParam(query, key),
      )
      .join("&");
  }

  protected addQueryParams(rawQuery?: QueryParamsType): string {
    const queryString = this.toQueryString(rawQuery);
    return queryString ? `?${queryString}` : "";
  }

  private contentFormatters: Record<ContentType, (input: any) => any> = {
    [ContentType.Json]: (input: any) =>
      input !== null && (typeof input === "object" || typeof input === "string")
        ? JSON.stringify(input)
        : input,
    [ContentType.JsonApi]: (input: any) =>
      input !== null && (typeof input === "object" || typeof input === "string")
        ? JSON.stringify(input)
        : input,
    [ContentType.Text]: (input: any) =>
      input !== null && typeof input !== "string"
        ? JSON.stringify(input)
        : input,
    [ContentType.FormData]: (input: any) =>
      Object.keys(input || {}).reduce((formData, key) => {
        const property = input[key];
        formData.append(
          key,
          property instanceof Blob
            ? property
            : typeof property === "object" && property !== null
              ? JSON.stringify(property)
              : `${property}`,
        );
        return formData;
      }, new FormData()),
    [ContentType.UrlEncoded]: (input: any) => this.toQueryString(input),
  };

  protected mergeRequestParams(
    params1: RequestParams,
    params2?: RequestParams,
  ): RequestParams {
    return {
      ...this.baseApiParams,
      ...params1,
      ...(params2 || {}),
      headers: {
        ...(this.baseApiParams.headers || {}),
        ...(params1.headers || {}),
        ...((params2 && params2.headers) || {}),
      },
    };
  }

  protected createAbortSignal = (
    cancelToken: CancelToken,
  ): AbortSignal | undefined => {
    if (this.abortControllers.has(cancelToken)) {
      const abortController = this.abortControllers.get(cancelToken);
      if (abortController) {
        return abortController.signal;
      }
      return void 0;
    }

    const abortController = new AbortController();
    this.abortControllers.set(cancelToken, abortController);
    return abortController.signal;
  };

  public abortRequest = (cancelToken: CancelToken) => {
    const abortController = this.abortControllers.get(cancelToken);

    if (abortController) {
      abortController.abort();
      this.abortControllers.delete(cancelToken);
    }
  };

  public request = async <T = any, E = any>({
    body,
    secure,
    path,
    type,
    query,
    format,
    baseUrl,
    cancelToken,
    ...params
  }: FullRequestParams): Promise<HttpResponse<T, E>> => {
    const secureParams =
      ((typeof secure === "boolean" ? secure : this.baseApiParams.secure) &&
        this.securityWorker &&
        (await this.securityWorker(this.securityData))) ||
      {};
    const requestParams = this.mergeRequestParams(params, secureParams);
    const queryString = query && this.toQueryString(query);
    const payloadFormatter = this.contentFormatters[type || ContentType.Json];
    const responseFormat = format || requestParams.format;

    return this.customFetch(
      `${baseUrl || this.baseUrl || ""}${path}${queryString ? `?${queryString}` : ""}`,
      {
        ...requestParams,
        headers: {
          ...(requestParams.headers || {}),
          ...(type && type !== ContentType.FormData
            ? { "Content-Type": type }
            : {}),
        },
        signal:
          (cancelToken
            ? this.createAbortSignal(cancelToken)
            : requestParams.signal) || null,
        body:
          typeof body === "undefined" || body === null
            ? null
            : payloadFormatter(body),
      },
    ).then(async (response) => {
      const r = response.clone() as HttpResponse<T, E>;
      r.data = null as unknown as T;
      r.error = null as unknown as E;

      const data = !responseFormat
        ? r
        : await response[responseFormat]()
            .then((data) => {
              if (r.ok) {
                r.data = data;
              } else {
                r.error = data;
              }
              return r;
            })
            .catch((e) => {
              r.error = e;
              return r;
            });

      if (cancelToken) {
        this.abortControllers.delete(cancelToken);
      }

      if (!response.ok) throw data;
      return data;
    });
  };
}

/**
 * @title Cyclops CS Backend API
 * @version 0.1
 * @baseUrl /
 * @contact
 *
 * Backend sidecar for the cyclops-cs SPA — Keycloak-authenticated key management, service proxies (k8s / orch / svc), namespace management, and deprecated gateway / batch / label routes that now return 410 Gone. All pool operations use OSGymSandboxClaim CRs (Path B).
 */
export class Api<
  SecurityDataType extends unknown,
> extends HttpClient<SecurityDataType> {
  /**
   * No description
   *
   * @tags health
   * @name HealthzList
   * @summary Liveness/readiness probe
   * @request GET:/healthz
   */
  healthzList = (params: RequestParams = {}) =>
    this.request<HealthzListData, any>({
      path: `/healthz`,
      method: "GET",
      format: "json",
      ...params,
    });

  batch = {
    /**
     * @description Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
     *
     * @tags batch
     * @name BatchLanesCreate
     * @summary Deprecated batch lane acquisition route
     * @request POST:/api/batch/{pool}/lanes
     * @deprecated
     * @secure
     */
    batchLanesCreate: (pool: string, params: RequestParams = {}) =>
      this.request<any, BatchLanesCreateError>({
        path: `/api/batch/${pool}/lanes`,
        method: "POST",
        secure: true,
        ...params,
      }),

    /**
     * @description Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
     *
     * @tags batch
     * @name BatchLanesDelete
     * @summary Deprecated batch lane release route
     * @request DELETE:/api/batch/{pool}/lanes
     * @deprecated
     * @secure
     */
    batchLanesDelete: (pool: string, params: RequestParams = {}) =>
      this.request<any, BatchLanesDeleteError>({
        path: `/api/batch/${pool}/lanes`,
        method: "DELETE",
        secure: true,
        ...params,
      }),

    /**
     * @description Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
     *
     * @tags batch
     * @name BatchSubmitCreate
     * @summary Deprecated batch submission route
     * @request POST:/api/batch/{pool}/submit
     * @deprecated
     * @secure
     */
    batchSubmitCreate: (pool: string, params: RequestParams = {}) =>
      this.request<any, BatchSubmitCreateError>({
        path: `/api/batch/${pool}/submit`,
        method: "POST",
        secure: true,
        ...params,
      }),

    /**
     * @description Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
     *
     * @tags batch
     * @name BatchDelete
     * @summary Deprecated batch cancellation route
     * @request DELETE:/api/batch/{pool}/{id}
     * @deprecated
     * @secure
     */
    batchDelete: (pool: string, id: string, params: RequestParams = {}) =>
      this.request<any, BatchDeleteError>({
        path: `/api/batch/${pool}/${id}`,
        method: "DELETE",
        secure: true,
        ...params,
      }),

    /**
     * @description Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
     *
     * @tags batch
     * @name BatchResultsList
     * @summary Deprecated batch results route
     * @request GET:/api/batch/{pool}/{id}/results
     * @deprecated
     * @secure
     */
    batchResultsList: (pool: string, id: string, params: RequestParams = {}) =>
      this.request<any, BatchResultsListError>({
        path: `/api/batch/${pool}/${id}/results`,
        method: "GET",
        secure: true,
        ...params,
      }),

    /**
     * @description Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
     *
     * @tags batch
     * @name BatchStatusList
     * @summary Deprecated batch status route
     * @request GET:/api/batch/{pool}/{id}/status
     * @deprecated
     * @secure
     */
    batchStatusList: (pool: string, id: string, params: RequestParams = {}) =>
      this.request<any, BatchStatusListError>({
        path: `/api/batch/${pool}/${id}/status`,
        method: "GET",
        secure: true,
        ...params,
      }),
  };
  config = {
    /**
     * @description Returns OPA-evaluated feature flags for the authenticated SPA user. `admin` is true when the caller's JWT sub appears in input.flags.admin_subs.
     *
     * @tags config
     * @name ConfigList
     * @summary Per-user feature flags
     * @request GET:/api/config
     * @secure
     */
    configList: (params: RequestParams = {}) =>
      this.request<ConfigListData, ConfigListError>({
        path: `/api/config`,
        method: "GET",
        secure: true,
        format: "json",
        ...params,
      }),
  };
  gateway = {
    /**
     * @description DEPRECATED (CUA-609): The per-pool orchestrator HTTP layer has been removed. This route returns 410 Gone for all requests. All pool operations must use OSGymSandboxClaim CRs (Path B) directly.
     *
     * @tags gateway
     * @name GatewayDetail
     * @summary Reverse-proxy to the per-pool orchestrator (sole ingress path — CUA-527)
     * @request GET:/api/gateway/{name}/{path}
     * @secure
     */
    gatewayDetail: (name: string, path?: string, params: RequestParams = {}) =>
      this.request<GatewayDetailData, GatewayDetailError>({
        path: `/api/gateway/${name}/${path}`,
        method: "GET",
        secure: true,
        ...params,
      }),
  };
  k8S = {
    /**
     * @description Forwards requests to http://127.0.0.1:8001 (the kubectl-proxy sidecar) so the SPA can read K8s resources via the pod ServiceAccount. SPA-only; OPA-gated. EventList responses are filtered by the caller's OPA visible_events policy via auth.K8sEventFilterMiddleware (mounted in main.go's route chain).
     *
     * @tags passthrough
     * @name GetK8S
     * @summary Authenticated proxy to the in-pod kubectl-proxy sidecar
     * @request GET:/api/k8s/{path}
     * @secure
     */
    getK8S: (path: string, params: RequestParams = {}) =>
      this.request<GetK8SData, GetK8SError>({
        path: `/api/k8s/${path}`,
        method: "GET",
        secure: true,
        ...params,
      }),
  };
  keys = {
    /**
     * No description
     *
     * @tags keys
     * @name KeysList
     * @summary List the calling user's API keys
     * @request GET:/api/keys
     * @secure
     */
    keysList: (params: RequestParams = {}) =>
      this.request<KeysListData, KeysListError>({
        path: `/api/keys`,
        method: "GET",
        secure: true,
        format: "json",
        ...params,
      }),

    /**
     * @description Creates a Keycloak service-account client owned by the calling user. The returned `client_secret` is shown exactly once.
     *
     * @tags keys
     * @name KeysCreate
     * @summary Create a new API key
     * @request POST:/api/keys
     * @secure
     */
    keysCreate: (body: HandlersCreateKeyRequest, params: RequestParams = {}) =>
      this.request<KeysCreateData, KeysCreateError>({
        path: `/api/keys`,
        method: "POST",
        body: body,
        secure: true,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags keys
     * @name KeysDelete
     * @summary Revoke an API key by Keycloak client UUID
     * @request DELETE:/api/keys/{id}
     * @secure
     */
    keysDelete: (id: string, params: RequestParams = {}) =>
      this.request<KeysDeleteData, KeysDeleteError>({
        path: `/api/keys/${id}`,
        method: "DELETE",
        secure: true,
        ...params,
      }),
  };
  label = {
    /**
     * @description Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
     *
     * @tags batch
     * @name LabelDelete
     * @summary Deprecated label cancellation route
     * @request DELETE:/api/label/{pool}/{label}
     * @deprecated
     * @secure
     */
    labelDelete: (pool: string, label: string, params: RequestParams = {}) =>
      this.request<any, LabelDeleteError>({
        path: `/api/label/${pool}/${label}`,
        method: "DELETE",
        secure: true,
        ...params,
      }),

    /**
     * @description Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
     *
     * @tags batch
     * @name LabelBatchCreate
     * @summary Deprecated label batch submission route
     * @request POST:/api/label/{pool}/{label}/batch
     * @deprecated
     * @secure
     */
    labelBatchCreate: (
      pool: string,
      label: string,
      params: RequestParams = {},
    ) =>
      this.request<any, LabelBatchCreateError>({
        path: `/api/label/${pool}/${label}/batch`,
        method: "POST",
        secure: true,
        ...params,
      }),

    /**
     * @description Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
     *
     * @tags batch
     * @name LabelResultsList
     * @summary Deprecated label results route
     * @request GET:/api/label/{pool}/{label}/results
     * @deprecated
     * @secure
     */
    labelResultsList: (
      pool: string,
      label: string,
      params: RequestParams = {},
    ) =>
      this.request<any, LabelResultsListError>({
        path: `/api/label/${pool}/${label}/results`,
        method: "GET",
        secure: true,
        ...params,
      }),

    /**
     * @description Route is deprecated and unavailable. Returns 410 Gone for every request. The orchestrator-backed batch surface is retired; callers must migrate to the replacement flow.
     *
     * @tags batch
     * @name LabelStatusList
     * @summary Deprecated label status route
     * @request GET:/api/label/{pool}/{label}/status
     * @deprecated
     * @secure
     */
    labelStatusList: (
      pool: string,
      label: string,
      params: RequestParams = {},
    ) =>
      this.request<any, LabelStatusListError>({
        path: `/api/label/${pool}/${label}/status`,
        method: "GET",
        secure: true,
        ...params,
      }),
  };
  namespaces = {
    /**
     * @description Returns namespaces owned by the caller's Capsule Tenant. The list is scoped by a capsule.clastix.io/tenant=<tenant> label selector built from the authenticated subject, so it stays fail-closed even when Capsule Proxy isn't filtering.
     *
     * @tags namespaces
     * @name NamespacesList
     * @summary List the calling user's namespaces
     * @request GET:/api/namespaces
     * @secure
     */
    namespacesList: (params: RequestParams = {}) =>
      this.request<NamespacesListData, NamespacesListError>({
        path: `/api/namespaces`,
        method: "GET",
        secure: true,
        format: "json",
        ...params,
      }),

    /**
     * @description Creates a K8s namespace via impersonation. Capsule's webhook intercepts the creation and assigns it to the user's Tenant.
     *
     * @tags namespaces
     * @name NamespacesCreate
     * @summary Create a namespace for the calling user
     * @request POST:/api/namespaces
     * @secure
     */
    namespacesCreate: (
      body: HandlersCreateNamespaceRequest,
      params: RequestParams = {},
    ) =>
      this.request<NamespacesCreateData, NamespacesCreateError>({
        path: `/api/namespaces`,
        method: "POST",
        body: body,
        secure: true,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * @description Deletes a K8s namespace via impersonation. Capsule blocks deletion if the namespace doesn't belong to the user's Tenant.
     *
     * @tags namespaces
     * @name NamespacesDelete
     * @summary Delete a namespace owned by the calling user
     * @request DELETE:/api/namespaces/{name}
     * @secure
     */
    namespacesDelete: (name: string, params: RequestParams = {}) =>
      this.request<NamespacesDeleteData, NamespacesDeleteError>({
        path: `/api/namespaces/${name}`,
        method: "DELETE",
        secure: true,
        ...params,
      }),
  };
  orch = {
    /**
     * @description Resolves <service>.<namespace>.svc.cluster.local at request time (in-cluster DNS). The caller must hold RBAC in {namespace} (verified via an impersonated RoleBinding probe); OPA additionally validates that namespace and service look like DNS-1123 labels.
     *
     * @tags passthrough
     * @name OrchDetail
     * @summary SPA-authenticated proxy to a per-namespace orchestrator service
     * @request GET:/api/orch/{namespace}/{service}/{path}
     * @secure
     */
    orchDetail: (
      namespace: string,
      service: string,
      path: string,
      params: RequestParams = {},
    ) =>
      this.request<OrchDetailData, OrchDetailError>({
        path: `/api/orch/${namespace}/${service}/${path}`,
        method: "GET",
        secure: true,
        ...params,
      }),
  };
  poolTemplates = {
    /**
     * @description Returns every pool template owned by the calling user, newest first.
     *
     * @tags pool-templates
     * @name PoolTemplatesList
     * @summary List the calling user's pool templates
     * @request GET:/api/pool-templates
     * @secure
     */
    poolTemplatesList: (params: RequestParams = {}) =>
      this.request<PoolTemplatesListData, PoolTemplatesListError>({
        path: `/api/pool-templates`,
        method: "GET",
        secure: true,
        format: "json",
        ...params,
      }),

    /**
     * @description Stores the given pool config under a name owned by the calling user. Re-saving an existing name overwrites the config while preserving the original creation time. Use GET /api/pool-templates to list them and seed a new pool.
     *
     * @tags pool-templates
     * @name PoolTemplatesCreate
     * @summary Save a pool config as a reusable template
     * @request POST:/api/pool-templates
     * @secure
     */
    poolTemplatesCreate: (
      body: HandlersCreatePoolTemplateRequest,
      params: RequestParams = {},
    ) =>
      this.request<PoolTemplatesCreateData, PoolTemplatesCreateError>({
        path: `/api/pool-templates`,
        method: "POST",
        body: body,
        secure: true,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags pool-templates
     * @name PoolTemplatesDetail
     * @summary Get one of the calling user's pool templates
     * @request GET:/api/pool-templates/{name}
     * @secure
     */
    poolTemplatesDetail: (name: string, params: RequestParams = {}) =>
      this.request<PoolTemplatesDetailData, PoolTemplatesDetailError>({
        path: `/api/pool-templates/${name}`,
        method: "GET",
        secure: true,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags pool-templates
     * @name PoolTemplatesDelete
     * @summary Delete one of the calling user's pool templates
     * @request DELETE:/api/pool-templates/{name}
     * @secure
     */
    poolTemplatesDelete: (name: string, params: RequestParams = {}) =>
      this.request<PoolTemplatesDeleteData, PoolTemplatesDeleteError>({
        path: `/api/pool-templates/${name}`,
        method: "DELETE",
        secure: true,
        ...params,
      }),
  };
  svc = {
    /**
     * @description Proxies to {service}.{namespace}.svc.cluster.local:80. Per-key tokens are bound to their `namespace` claim; all other principals (SPA, user keys, oauth2-proxy browser sessions) must hold RBAC in {namespace}, verified via an impersonated RoleBinding probe. Strips Authorization before forwarding.
     *
     * @tags gateway
     * @name GetSvc
     * @summary Authenticated reverse proxy to a K8s Service in a namespace the caller owns
     * @request GET:/api/svc/{namespace}/{service}/{path}
     * @secure
     */
    getSvc: (
      namespace: string,
      service: string,
      path?: string,
      params: RequestParams = {},
    ) =>
      this.request<GetSvcData, GetSvcError>({
        path: `/api/svc/${namespace}/${service}/${path}`,
        method: "GET",
        secure: true,
        ...params,
      }),
  };
  userKeys = {
    /**
     * No description
     *
     * @tags user-keys
     * @name UserKeysList
     * @summary List the calling user's API keys
     * @request GET:/api/user-keys
     * @secure
     */
    userKeysList: (params: RequestParams = {}) =>
      this.request<UserKeysListData, UserKeysListError>({
        path: `/api/user-keys`,
        method: "GET",
        secure: true,
        format: "json",
        ...params,
      }),

    /**
     * @description Creates a Keycloak service-account client that acts on behalf of the calling user. The client_secret is returned exactly once; it cannot be retrieved later.
     *
     * @tags user-keys
     * @name UserKeysCreate
     * @summary Create a per-user API key
     * @request POST:/api/user-keys
     * @secure
     */
    userKeysCreate: (
      body: HandlersCreateUserKeyRequest,
      params: RequestParams = {},
    ) =>
      this.request<UserKeysCreateData, UserKeysCreateError>({
        path: `/api/user-keys`,
        method: "POST",
        body: body,
        secure: true,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags user-keys
     * @name UserKeysDelete
     * @summary Revoke a per-user API key
     * @request DELETE:/api/user-keys/{id}
     * @secure
     */
    userKeysDelete: (id: string, params: RequestParams = {}) =>
      this.request<UserKeysDeleteData, UserKeysDeleteError>({
        path: `/api/user-keys/${id}`,
        method: "DELETE",
        secure: true,
        ...params,
      }),
  };
}
