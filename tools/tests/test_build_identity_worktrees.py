"""Exercise the actual build identity module with Cargo shared across worktrees."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[2]

class SharedCargoIdentityTests(unittest.TestCase):
    def test_shared_target_updates_revision_for_both_worktrees(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get('DSH_TEST_TEMP_DIR'), prefix='identity-worktrees-') as directory:
            root = Path(directory)
            repo = root/'repo'
            repo.mkdir()
            def run(args, cwd=repo, env=None):
                return subprocess.check_output(args, cwd=cwd, env=env, stderr=subprocess.STDOUT, creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0)).decode('utf-8').strip()
            run(['git', 'init', '-q'])
            run(['git', 'config', 'user.name', 'Fixture'])
            run(['git', 'config', 'user.email', 'fixture@example.invalid'])
            (repo/'Cargo.toml').write_text('[package]\nname="identity-fixture"\nversion="0.1.0"\nedition="2024"\n', encoding='utf-8')
            (repo/'src').mkdir()
            (repo/'src/main.rs').write_text('fn main(){ println!("{} {} {} {}",env!("DSH_BUILD_REVISION"),env!("DSH_BUILD_DIRTY"),env!("DSH_NATIVE_REVISION"),env!("DSH_NATIVE_DIRTY")); }', encoding='utf-8')
            (repo/'build_identity.rs').write_bytes((SOURCE/'crates/host/dsh-cli/build_identity.rs').read_bytes())
            (repo/'build.rs').write_text('mod build_identity; fn main(){let path=std::env::var("CARGO_MANIFEST_DIR").unwrap();let path=std::path::Path::new(&path);build_identity::emit(path);build_identity::emit_named(path,"DSH_NATIVE");}', encoding='utf-8')
            (repo/'.gitignore').write_text('Cargo.lock\n', encoding='utf-8')
            run(['git', 'add', '.'])
            run(['git', 'commit', '-qm', 'first'])
            first = run(['git', 'rev-parse', 'HEAD'])
            tree = root/'second'
            run(['git', 'worktree', 'add', '--detach', str(tree), first])
            (tree/'source-marker').write_text('different source revision', encoding='utf-8')
            run(['git', 'add', '.'], tree)
            run(['git', 'commit', '-qm', 'second'], tree)
            second = run(['git', 'rev-parse', 'HEAD'], tree)
            env = dict(os.environ, CARGO_TARGET_DIR=str(root/'target'), CARGO_BUILD_JOBS='2')
            env.pop('DSH_BUILD_SOURCE_ID', None)
            for source, expected in [(repo, first), (tree, second), (repo, first)]:
                run(['cargo', 'build', '--offline', '-q'], source, env)
                exe = root/'target/debug'/('identity-fixture.exe' if os.name=='nt' else 'identity-fixture')
                self.assertEqual(run([str(exe)], source), f'{expected} false {expected} false')

if __name__ == '__main__':
    unittest.main()
