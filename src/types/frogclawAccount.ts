export interface ToolStatus {
  id: string;
  name: string;
  installed: boolean;
  version: string | null;
  path: string | null;
  installable: boolean;
  needs_upgrade: boolean;
}

export interface HomeToolsStatus {
  tools: ToolStatus[];
}

export interface InstallResult {
  success: boolean;
  stdout: string;
  stderr: string;
  message: string;
  log_file?: string | null;
}

export interface FrogclawUserData {
  id: number;
  username: string;
  display_name: string;
  role: number;
  status: number;
  group: string;
}

export interface FrogclawToken {
  id: number;
  key: string;
  name: string;
  status: number;
  remain_quota: number;
  unlimited_quota: boolean;
  group: string;
}

export interface FrogclawSystemProvider {
  id: number;
  name: string;
  provider_key: string;
  api_mode: string;
  needs_v1_suffix: boolean;
  base_url: string;
  default_model: string | null;
  use_site_token: boolean;
  token_group: string;
}

export interface FrogclawCliProvider {
  id: number;
  name: string;
  provider_type: string;
  base_url: string | null;
  api_key: string | null;
  settings_config: string | null;
  is_default: boolean | null;
  created_time: number | null;
  updated_time: number | null;
}

export interface FrogclawPricingModel {
  model_name: string;
  description?: string;
  tags?: string;
  vendor_id?: number | null;
  enable_groups: string[];
  supported_endpoint_types: string[];
}

export interface FrogclawPricingVendor {
  id: number;
  name: string;
  description?: string;
  icon?: string;
}

export interface FrogclawLoginSession {
  user: FrogclawUserData;
  tokens: FrogclawToken[];
  system_providers: FrogclawSystemProvider[];
  pricing_models: FrogclawPricingModel[];
  pricing_vendors: FrogclawPricingVendor[];
}

export interface FrogclawConfiguredProvider {
  provider_id: string;
  name: string;
  provider_type: string;
  model_count: number;
  token_id: number;
  token_name: string;
  token_group: string;
  created_provider: boolean;
  updated_key: boolean;
}

export interface FrogclawConfigureResult {
  session: FrogclawLoginSession;
  configured_providers: FrogclawConfiguredProvider[];
  selected_token_id: number | null;
}
