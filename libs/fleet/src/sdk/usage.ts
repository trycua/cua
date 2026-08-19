import { getToken } from "../auth/keycloak"
export type UsageTimeframe="24h"|"7d"|"30d"
export interface UsageMetricTotals{consumed:number;provisioned:number}
export interface UsagePoolSummary{id:string;name:string;cpu:UsageMetricTotals;memory:UsageMetricTotals}
export interface UsageOverviewResponse{data_as_of:string;partial:boolean;pools:UsagePoolSummary[]}
export interface UsageBucket{start:string;end:string;cpu_consumed:number;cpu_provisioned:number;memory_consumed:number;memory_provisioned:number}
export interface UsagePoolDetailResponse{data_as_of:string;partial:boolean;pool:{id:string;name:string};buckets:UsageBucket[]}
async function request<T>(path:string,p:URLSearchParams):Promise<T>{const token=await getToken();const r=await fetch(`${path}?${p}`,{headers:{Authorization:token?`Bearer ${token}`:""}});if(!r.ok){const b=await r.json().catch(()=>null) as {error?:string}|null;throw new Error(b?.error??`usage request failed: ${r.status}`)}return r.json() as Promise<T>}
function params(timeframe:UsageTimeframe,subject?:string){const p=new URLSearchParams({timeframe});if(subject)p.set("subject",subject);return p}
export const usageApi={overview:(t:UsageTimeframe,s?:string)=>request<UsageOverviewResponse>("/api/usage/overview",params(t,s)),pool:(t:UsageTimeframe,poolId:string,s?:string)=>{const p=params(t,s);p.set("pool",poolId);return request<UsagePoolDetailResponse>("/api/usage/pool",p)}}
