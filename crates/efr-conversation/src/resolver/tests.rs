use efr_protocol::{ProjectId, Scope};
use efr_scope::{Basis, Git, Home};
use efr_stdx::id::uuid_v7;
use efr_test_support::{TestClock, TestDirs, TestRng};
use pretty_assertions::assert_eq;

use super::{GitScopeResolver, ScopeResolver};

fn resolver(dirs: &TestDirs) -> GitScopeResolver {
    let home = Home::new(dirs.home()).expect("home");
    GitScopeResolver::new(home, Git::new(TestClock::new().shared()).isolated())
}

#[tokio::test]
async fn the_home_directory_is_the_machine_scope() {
    let dirs = TestDirs::new().expect("directories");
    let derivation = resolver(&dirs).resolve(dirs.home()).await;
    assert_eq!(derivation.scope, Scope::Machine);
    assert_eq!(derivation.basis, Basis::HomeOrRoot);
}

#[tokio::test]
async fn a_directory_in_a_registered_project_is_that_project_read_every_turn() {
    let dirs = TestDirs::new().expect("directories");
    let project = dirs.create_dir("home/project/src").expect("project");
    let registry = dirs.dirs().config().join("projects.toml");
    let resolver = resolver(&dirs).with_registry(&registry);

    let before = resolver.resolve(&project).await;
    let id = ProjectId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(4)));
    let root = dirs.home().join("project");
    std::fs::write(
        &registry,
        format!("[[project]]\nid = \"{id}\"\nroot = \"{}\"\n", root.display()),
    )
    .expect("registry");
    let after = resolver.resolve(&project).await;

    assert_eq!(before.scope, Scope::Machine);
    assert_eq!(after.scope, Scope::Project(id));
    assert_eq!(after.basis, Basis::Registered);
}

#[tokio::test]
async fn a_broken_registry_or_a_relative_directory_falls_back_to_the_machine() {
    let dirs = TestDirs::new().expect("directories");
    let registry = dirs.dirs().config().join("projects.toml");
    std::fs::write(&registry, "not toml [").expect("registry");
    let resolver = resolver(&dirs).with_registry(&registry);

    assert_eq!(resolver.resolve(dirs.home()).await.scope, Scope::Machine);
    assert_eq!(resolver.resolve("relative/dir".as_ref()).await.scope, Scope::Machine);
}
