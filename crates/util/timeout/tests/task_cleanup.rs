use dsh_timeout::AbortTaskOnDrop;

#[tokio::test]
async fn dropping_owner_cancels_and_releases_the_helper_task() {
    struct Dropped(Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            let _ = self.0.take().unwrap().send(());
        }
    }
    let (started, ready) = tokio::sync::oneshot::channel();
    let (released, done) = tokio::sync::oneshot::channel();
    let helper = tokio::spawn(async move {
        let _release = Dropped(Some(released));
        started.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    let owner = AbortTaskOnDrop::new(helper);
    ready.await.unwrap();
    drop(owner);
    tokio::time::timeout(std::time::Duration::from_secs(1), done)
        .await
        .unwrap()
        .unwrap();
}
