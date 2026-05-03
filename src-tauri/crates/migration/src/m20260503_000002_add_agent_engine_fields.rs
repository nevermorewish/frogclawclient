use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("agent_sessions"))
                    .add_column(
                        ColumnDef::new(Alias::new("engine_kind"))
                            .string()
                            .not_null()
                            .default("frog_agent"),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("agent_sessions"))
                    .add_column(ColumnDef::new(Alias::new("engine_session_id")).string().null())
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("agent_sessions"))
                    .add_column(ColumnDef::new(Alias::new("engine_context_json")).text().null())
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("agent_sessions"))
                    .add_column(
                        ColumnDef::new(Alias::new("engine_context_backup_json"))
                            .text()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("agent_sessions"))
                    .add_column(ColumnDef::new(Alias::new("engine_error")).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            "engine_error",
            "engine_context_backup_json",
            "engine_context_json",
            "engine_session_id",
            "engine_kind",
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("agent_sessions"))
                        .drop_column(Alias::new(column))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}
