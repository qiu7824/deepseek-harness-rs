import hashlib,json,pathlib,tempfile,unittest,zipfile
from unittest.mock import patch
from tools import stage_node_runtime as runtime

class NodeRuntimePackageTests(unittest.TestCase):
    def fixture(self,root):
        cache=root/'cache';cache.mkdir();(root/'tools').mkdir()
        name='node-v26.8.2-win-x64.zip';archive=cache/name
        with zipfile.ZipFile(archive,'w') as package:
            package.writestr('node-v26.8.2-win-x64/node.exe',b'fixture-node')
            package.writestr('node-v26.8.2-win-x64/LICENSE',b'fixture-license')
        digest=hashlib.sha256(archive.read_bytes()).hexdigest()
        (root/'tools/node_runtime_lock.json').write_text(json.dumps({'version':'26.8.2','baseUrl':'https://invalid.example/','archives':{name:digest}}))
        return cache,archive,digest

    def test_pinned_archive_stages_runtime_license_and_binary_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=pathlib.Path(temporary);cache,archive,digest=self.fixture(root)
            with patch.object(runtime,'ROOT',root),patch.object(runtime.urllib.request,'urlopen',side_effect=AssertionError('cache must be reused')):
                runtime.stage_node_runtime(root/'stage','windows','x86_64',cache)
            output=root/'stage/runtime/node'
            self.assertEqual((output/'node.exe').read_bytes(),b'fixture-node')
            self.assertEqual((output/'LICENSE').read_bytes(),b'fixture-license')
            identity=json.loads((output/'IDENTITY.json').read_text())
            self.assertEqual(identity['archiveSha256'],digest)
            self.assertEqual(identity['binarySha256'],hashlib.sha256(b'fixture-node').hexdigest())

    def test_corrupt_cached_archive_is_rejected_before_staging(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=pathlib.Path(temporary);cache,archive,_=self.fixture(root)
            archive.write_bytes(archive.read_bytes()+b'tampered')
            with patch.object(runtime,'ROOT',root),self.assertRaisesRegex(ValueError,'checksum mismatch'):
                runtime.stage_node_runtime(root/'stage','windows','x86_64',cache)
            self.assertFalse((root/'stage/runtime/node/node.exe').exists())
