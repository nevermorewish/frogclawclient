use sea_orm::sea_query::Expr;
use sea_orm::*;
use sha2::{Digest, Sha256};

use crate::entity::{conversations, memory_items, memory_namespaces, models, providers};
use crate::error::{FrogClawClientError, Result};
use crate::types::{
    CreateMemoryItemInput, CreateMemoryNamespaceInput, MemoryItem, MemoryNamespace,
    ProjectMemoryProfile, UpdateMemoryItemInput, UpdateMemoryNamespaceInput,
};
use crate::utils::gen_id;

fn model_to_namespace(m: memory_namespaces::Model) -> MemoryNamespace {
    MemoryNamespace {
        id: m.id,
        name: m.name,
        scope: m.scope,
        embedding_provider: m.embedding_provider,
        embedding_dimensions: m.embedding_dimensions,
        retrieval_threshold: m.retrieval_threshold,
        retrieval_top_k: m.retrieval_top_k,
        icon_type: m.icon_type,
        icon_value: m.icon_value,
        sort_order: m.sort_order,
    }
}

fn model_to_item(m: memory_items::Model) -> MemoryItem {
    MemoryItem {
        id: m.id,
        namespace_id: m.namespace_id,
        title: m.title,
        content: m.content,
        source: m.source,
        index_status: m.index_status,
        index_error: m.index_error,
        updated_at: m.updated_at,
    }
}

fn normalize_project_path(project_path: &str) -> String {
    project_path.trim().replace('\\', "/").trim_end_matches('/').to_string()
}

fn project_namespace_id(project_path: &str) -> String {
    let normalized = normalize_project_path(project_path).to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    format!("project_mem_{}", hex::encode(digest)[..24].to_string())
}

fn fallback_project_name(project_path: &str) -> String {
    let normalized = normalize_project_path(project_path);
    normalized
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("默认项目")
        .to_string()
}

async fn default_project_embedding_provider(db: &DatabaseConnection) -> Result<Option<String>> {
    let settings = crate::repo::settings::get_settings(db).await?;
    if let (Some(provider_id), Some(model_id)) = (
        settings.default_embedding_provider_id.as_deref(),
        settings.default_embedding_model_id.as_deref(),
    ) {
        if !provider_id.trim().is_empty() && !model_id.trim().is_empty() {
            return Ok(Some(format!("{}::{}", provider_id, model_id)));
        }
    }

    let rows = models::Entity::find()
        .inner_join(providers::Entity)
        .filter(providers::Column::Enabled.eq(1))
        .filter(models::Column::Enabled.eq(1))
        .all(db)
        .await?;

    let preferred = rows
        .iter()
        .find(|model| model.model_id.eq_ignore_ascii_case("BAAI/bge-m3"))
        .or_else(|| {
            rows.iter().find(|model| {
                let id = model.model_id.to_lowercase();
                id.ends_with("/bge-m3") || id == "bge-m3" || id.contains("baai/bge-m3")
            })
        });

    Ok(preferred.map(|model| format!("{}::{}", model.provider_id, model.model_id)))
}

async fn count_items_by_status(
    db: &DatabaseConnection,
    namespace_id: &str,
    status: Option<&str>,
) -> Result<i64> {
    let mut query = memory_items::Entity::find()
        .filter(memory_items::Column::NamespaceId.eq(namespace_id));
    if let Some(status) = status {
        query = query.filter(memory_items::Column::IndexStatus.eq(status));
    }
    Ok(query.count(db).await? as i64)
}

async fn namespace_to_project_profile(
    db: &DatabaseConnection,
    project_path: &str,
    project_name: &str,
    ns: MemoryNamespace,
) -> Result<ProjectMemoryProfile> {
    let item_count = count_items_by_status(db, &ns.id, None).await?;
    let pending_count = count_items_by_status(db, &ns.id, Some("pending")).await?
        + count_items_by_status(db, &ns.id, Some("indexing")).await?
        + count_items_by_status(db, &ns.id, Some("skipped")).await?;
    let failed_count = count_items_by_status(db, &ns.id, Some("failed")).await?;
    Ok(ProjectMemoryProfile {
        project_path: normalize_project_path(project_path),
        project_name: project_name.to_string(),
        namespace_id: ns.id,
        enabled: true,
        embedding_provider: ns.embedding_provider,
        embedding_dimensions: ns.embedding_dimensions,
        retrieval_threshold: ns.retrieval_threshold,
        retrieval_top_k: ns.retrieval_top_k,
        item_count,
        pending_count,
        failed_count,
    })
}

pub async fn list_namespaces(db: &DatabaseConnection) -> Result<Vec<MemoryNamespace>> {
    let models = memory_namespaces::Entity::find()
        .order_by_asc(memory_namespaces::Column::SortOrder)
        .all(db)
        .await?;

    Ok(models.into_iter().map(model_to_namespace).collect())
}

pub async fn get_namespace(db: &DatabaseConnection, id: &str) -> Result<MemoryNamespace> {
    let model = memory_namespaces::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| FrogClawClientError::NotFound(format!("MemoryNamespace {}", id)))?;

    Ok(model_to_namespace(model))
}

pub async fn ensure_project_profile(
    db: &DatabaseConnection,
    project_path: &str,
    project_name: Option<&str>,
) -> Result<ProjectMemoryProfile> {
    let normalized_path = normalize_project_path(project_path);
    let name = project_name
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|| fallback_project_name(&normalized_path));
    let namespace_id = project_namespace_id(&normalized_path);

    let existing = memory_namespaces::Entity::find_by_id(&namespace_id)
        .one(db)
        .await?;

    let ns = if let Some(model) = existing {
        let mut ns = model_to_namespace(model);
        if ns.embedding_provider.is_none() {
            if let Some(default_embedding_provider) = default_project_embedding_provider(db).await? {
                ns = update_namespace(
                    db,
                    &namespace_id,
                    UpdateMemoryNamespaceInput {
                        name: None,
                        embedding_provider: Some(default_embedding_provider),
                        update_embedding_provider: true,
                        embedding_dimensions: None,
                        update_embedding_dimensions: false,
                        retrieval_threshold: None,
                        update_retrieval_threshold: false,
                        retrieval_top_k: None,
                        update_retrieval_top_k: false,
                        icon_type: None,
                        icon_value: None,
                        update_icon: false,
                        sort_order: None,
                    },
                )
                .await?;
            }
        }
        ns
    } else {
        let default_embedding_provider = default_project_embedding_provider(db).await?;
        let am = memory_namespaces::ActiveModel {
            id: Set(namespace_id.clone()),
            name: Set(name.clone()),
            scope: Set("project".to_string()),
            embedding_provider: Set(default_embedding_provider),
            embedding_dimensions: Set(None),
            retrieval_threshold: Set(Some(0.35)),
            retrieval_top_k: Set(Some(6)),
            icon_type: Set(Some("lucide".to_string())),
            icon_value: Set(Some("FolderOpen".to_string())),
            sort_order: Set(0),
        };
        am.insert(db).await?;
        get_namespace(db, &namespace_id).await?
    };

    namespace_to_project_profile(db, &normalized_path, &name, ns).await
}

pub async fn list_project_profiles(
    db: &DatabaseConnection,
    default_project_path: &str,
) -> Result<Vec<ProjectMemoryProfile>> {
    let mut projects: Vec<(String, String)> = vec![(
        normalize_project_path(default_project_path),
        fallback_project_name(default_project_path),
    )];

    let conversations = conversations::Entity::find()
        .filter(conversations::Column::WorkingDirectory.is_not_null())
        .all(db)
        .await?;

    for conversation in conversations {
        let Some(path) = conversation.working_directory else {
            continue;
        };
        let normalized = normalize_project_path(&path);
        if normalized.is_empty() || projects.iter().any(|(p, _)| p == &normalized) {
            continue;
        }
        let name = conversation
            .project_name
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| fallback_project_name(&normalized));
        projects.push((normalized, name));
    }

    let mut out = Vec::with_capacity(projects.len());
    for (path, name) in projects {
        out.push(ensure_project_profile(db, &path, Some(&name)).await?);
    }
    out.sort_by(|a, b| a.project_name.to_lowercase().cmp(&b.project_name.to_lowercase()));
    Ok(out)
}

pub async fn get_project_profile(
    db: &DatabaseConnection,
    project_path: &str,
    project_name: Option<&str>,
) -> Result<ProjectMemoryProfile> {
    ensure_project_profile(db, project_path, project_name).await
}

pub async fn update_project_profile(
    db: &DatabaseConnection,
    project_path: &str,
    project_name: Option<&str>,
    input: UpdateMemoryNamespaceInput,
) -> Result<ProjectMemoryProfile> {
    let profile = ensure_project_profile(db, project_path, project_name).await?;
    let ns = update_namespace(db, &profile.namespace_id, input).await?;
    namespace_to_project_profile(db, project_path, &profile.project_name, ns).await
}

pub async fn list_project_items(
    db: &DatabaseConnection,
    project_path: &str,
    project_name: Option<&str>,
) -> Result<Vec<MemoryItem>> {
    let profile = ensure_project_profile(db, project_path, project_name).await?;
    list_items(db, &profile.namespace_id).await
}

pub async fn create_namespace(
    db: &DatabaseConnection,
    input: CreateMemoryNamespaceInput,
) -> Result<MemoryNamespace> {
    let id = gen_id();

    let am = memory_namespaces::ActiveModel {
        id: Set(id.clone()),
        name: Set(input.name),
        scope: Set(input.scope),
        embedding_provider: Set(input.embedding_provider),
        embedding_dimensions: Set(input.embedding_dimensions),
        retrieval_threshold: Set(input.retrieval_threshold),
        retrieval_top_k: Set(input.retrieval_top_k),
        icon_type: Set(input.icon_type),
        icon_value: Set(input.icon_value),
        sort_order: Set(0),
    };

    am.insert(db).await?;

    get_namespace(db, &id).await
}

pub async fn delete_namespace(db: &DatabaseConnection, id: &str) -> Result<()> {
    let result = memory_namespaces::Entity::delete_by_id(id).exec(db).await?;

    if result.rows_affected == 0 {
        return Err(FrogClawClientError::NotFound(format!("MemoryNamespace {}", id)));
    }
    Ok(())
}

pub async fn update_namespace(
    db: &DatabaseConnection,
    id: &str,
    input: UpdateMemoryNamespaceInput,
) -> Result<MemoryNamespace> {
    let model = memory_namespaces::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| FrogClawClientError::NotFound(format!("MemoryNamespace {}", id)))?;

    let mut am: memory_namespaces::ActiveModel = model.clone().into();
    if let Some(name) = input.name {
        am.name = Set(name);
    }
    if input.update_embedding_provider {
        am.embedding_provider = Set(input.embedding_provider);
    }
    if input.update_embedding_dimensions {
        am.embedding_dimensions = Set(input.embedding_dimensions);
    }
    if input.update_retrieval_threshold {
        am.retrieval_threshold = Set(input.retrieval_threshold);
    }
    if input.update_retrieval_top_k {
        am.retrieval_top_k = Set(input.retrieval_top_k);
    }
    if input.update_icon {
        am.icon_type = Set(input.icon_type);
        am.icon_value = Set(input.icon_value);
    }
    if let Some(sort_order) = input.sort_order {
        am.sort_order = Set(sort_order);
    }
    am.update(db).await?;

    get_namespace(db, id).await
}

pub async fn reorder_namespaces(db: &DatabaseConnection, namespace_ids: &[String]) -> Result<()> {
    for (i, id) in namespace_ids.iter().enumerate() {
        memory_namespaces::Entity::update_many()
            .col_expr(memory_namespaces::Column::SortOrder, Expr::value(i as i32))
            .filter(memory_namespaces::Column::Id.eq(id))
            .exec(db)
            .await?;
    }
    Ok(())
}

pub async fn list_items(db: &DatabaseConnection, namespace_id: &str) -> Result<Vec<MemoryItem>> {
    let models = memory_items::Entity::find()
        .filter(memory_items::Column::NamespaceId.eq(namespace_id))
        .order_by_desc(memory_items::Column::UpdatedAt)
        .all(db)
        .await?;

    Ok(models.into_iter().map(model_to_item).collect())
}

pub async fn add_item(db: &DatabaseConnection, input: CreateMemoryItemInput) -> Result<MemoryItem> {
    let id = gen_id();
    let source = input.source.unwrap_or_else(|| "manual".to_string());

    let am = memory_items::ActiveModel {
        id: Set(id.clone()),
        namespace_id: Set(input.namespace_id),
        title: Set(input.title),
        content: Set(input.content),
        source: Set(source),
        ..Default::default()
    };

    am.insert(db).await?;

    let model = memory_items::Entity::find_by_id(&id)
        .one(db)
        .await?
        .ok_or_else(|| FrogClawClientError::NotFound(format!("MemoryItem {}", id)))?;

    Ok(model_to_item(model))
}

pub async fn delete_item(db: &DatabaseConnection, id: &str) -> Result<()> {
    let result = memory_items::Entity::delete_by_id(id).exec(db).await?;

    if result.rows_affected == 0 {
        return Err(FrogClawClientError::NotFound(format!("MemoryItem {}", id)));
    }
    Ok(())
}

pub async fn update_item(
    db: &DatabaseConnection,
    id: &str,
    input: UpdateMemoryItemInput,
) -> Result<MemoryItem> {
    let model = memory_items::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| FrogClawClientError::NotFound(format!("MemoryItem {}", id)))?;

    let mut am: memory_items::ActiveModel = model.into();
    if let Some(title) = input.title {
        am.title = Set(title);
    }
    if let Some(content) = input.content {
        am.content = Set(content);
        // Content changed — reset index status to pending
        am.index_status = Set("pending".to_string());
    }
    am.updated_at = Set(chrono::Utc::now().to_rfc3339());
    am.update(db).await?;

    let updated = memory_items::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| FrogClawClientError::NotFound(format!("MemoryItem {}", id)))?;

    Ok(model_to_item(updated))
}

pub async fn update_item_index_status(
    db: &DatabaseConnection,
    id: &str,
    status: &str,
    error: Option<&str>,
) -> Result<()> {
    let model = memory_items::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| FrogClawClientError::NotFound(format!("MemoryItem {}", id)))?;

    let mut am: memory_items::ActiveModel = model.into();
    am.index_status = Set(status.to_string());
    am.index_error = Set(error.map(|e| e.to_string()));
    am.update(db).await?;

    Ok(())
}
