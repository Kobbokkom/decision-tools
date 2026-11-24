use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

use super::registry::Script;

const OUTPUT_LIMIT: usize = 64 * 1024;

pub async fn run(script: &Script, directory: &Path, payload: &Value) -> Result<Value> {
    let program = directory.join(&script.program);
    let bytes = serde_json::to_vec(payload)?;
    let mut child = Command::new(program)
        .args(&script.args)
        .current_dir(directory)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .context("start registered script")?;
    let mut group = ProcessGroup(child.id().context("script PID unavailable")? as i32);
    let mut stdin = child.stdin.take().context("script stdin unavailable")?;
    let stdout = child.stdout.take().context("script stdout unavailable")?;

    let output = tokio::time::timeout(Duration::from_secs(script.timeout_secs), async {
        let (_, output) = tokio::try_join!(
            async {
                let result = stdin.write_all(&bytes).await;
                drop(stdin);
                match result {
                    Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
                    result => result,
                }
            },
            async {
                let mut output = Vec::new();
                stdout
                    .take((OUTPUT_LIMIT + 1) as u64)
                    .read_to_end(&mut output)
                    .await?;
                Ok(output)
            }
        )
        .context("script I/O failed")?;
        ensure!(
            output.len() <= OUTPUT_LIMIT,
            "script output exceeds 65536 bytes"
        );
        let status = child.wait().await.context("wait for registered script")?;
        ensure!(status.success(), "registered script exited with {status}");
        Ok::<_, anyhow::Error>(output)
    })
    .await
    .context("registered script timed out")??;
    group.terminate();
    serde_json::from_slice(&output).context("registered script must return JSON")
}

struct ProcessGroup(i32);

impl ProcessGroup {
    fn terminate(&mut self) {
        if self.0 > 0 {
            // The signal targets the process group created for this script.
            unsafe { libc::kill(-self.0, libc::SIGKILL) };
            self.0 = 0;
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.terminate();
    }
}
