//! `zfs commit` subcommand

use std::{
    io::Read,
    process::{Child, ChildStdout, Stdio},
};

use infinitree::Infinitree;
use zerostash_files::{Files, ZfsSnapshot};

use crate::prelude::*;

#[derive(Command, Debug)]
pub struct ZfsCommit {
    #[clap(flatten)]
    stash: StashArgs,

    /// Commit message to include in the changeset
    #[clap(short = 'm', long)]
    message: Option<String>,

    /// Name of the snapshot to commit (automatically appended to `zfs send`)
    #[clap(short = 'n', long)]
    name: String,

    /// Extra arguments to `zfs send`
    #[clap(name = "arguments")]
    #[arg(num_args(1..))]
    arguments: Vec<String>,
}

#[async_trait]
impl AsyncRunnable for ZfsCommit {
    /// Start the application.
    async fn run(&self) {
        let stash = self.stash.open();
        stash.load(stash.index().zfs_snapshots()).unwrap();

        let args = {
            let mut args = self.arguments.to_vec();
            args.push(self.name.clone());

            args
        };

        let mut child = execute_command(&args);
        let mut stdout = child.stdout.take().expect("failed to open stdout");

        store_stream_from_stdout(&stash, self.name.clone(), &mut stdout).await;

        let status = child.wait().expect("failed to wait for child process");
        let stderr = child.stderr.as_mut().expect("failed to open stderr");
        if !status.success() {
            let mut err = String::new();
            stderr.read_to_string(&mut err).unwrap();
            panic!("err: {}", err);
        }

        stash
            .commit(self.message.clone())
            .expect("failed to write metadata");
        stash.backend().sync().expect("failed to write to storage");
    }
}

fn execute_command(arguments: &[String]) -> Child {
    std::process::Command::new("zfs")
        .arg("send")
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to execute zfs send")
}

async fn store_stream_from_stdout(
    stash: &Infinitree<Files>,
    snapshot: String,
    stdout: &mut ChildStdout,
) {
    let snapshots = &stash.index().zfs_snapshots;

    if snapshots.get(&snapshot).is_some() {
        panic!("cannot overwrite existing snapshot");
    }

    let writer = stash.storage_writer().unwrap();
    let stream = abscissa_tokio::tokio::task::block_in_place(|| {
        ZfsSnapshot::from_stdout(writer, stdout).expect("failed to capture snapshot")
    });

    snapshots.insert(snapshot, stream);
}
