use std::path::Path;

use relay_core::{SqliteStore, StoredTask, list_stored_tasks};

pub(crate) async fn open_store(path: &Path) -> (Option<SqliteStore>, Vec<StoredTask>, String) {
    match SqliteStore::open(path).await {
        Ok(store) => match list_stored_tasks(&store).await {
            Ok(tasks) => {
                let feedback = format!("Database: {}", path.display());
                (Some(store), tasks, feedback)
            }
            Err(error) => (
                None,
                Vec::new(),
                format!(
                    "Database error at {}: {error}\nTask import is disabled until the database is fixed.",
                    path.display()
                ),
            ),
        },
        Err(error) => (
            None,
            Vec::new(),
            format!(
                "Database error at {}: {error}\nTask import is disabled until the database is fixed.",
                path.display()
            ),
        ),
    }
}
