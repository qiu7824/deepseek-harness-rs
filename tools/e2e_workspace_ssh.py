"""Exercise OpenSSH TCP forwarding against two isolated real Harness hosts.

The SSH fixture trusts only a generated host key and allows no remote commands.
It authenticates with a generated fixture key, never with user credentials.
"""
from __future__ import annotations

import argparse
import concurrent.futures
import contextlib
import getpass
import json
import os
import pathlib
import select
import socket
import sys
import subprocess
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

import paramiko

from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc
from e2e_sidebar_git_terminal import request as preview
from e2e_workspace_resources import file_has_text


class ForwardServer(paramiko.ServerInterface):
    def __init__(self, target, client_key, reject_auth=False):
        self.target = target
        self.client_key = client_key
        self.reject_auth = reject_auth

    def check_auth_none(self, username):
        return paramiko.AUTH_FAILED

    def check_auth_publickey(self, username, key):
        return paramiko.AUTH_SUCCESSFUL if not self.reject_auth and key.asbytes() == self.client_key.asbytes() else paramiko.AUTH_FAILED

    def get_allowed_auths(self, username):
        return "publickey"

    def check_channel_direct_tcpip_request(self, chanid, origin, destination):
        return paramiko.OPEN_SUCCEEDED if destination == ("127.0.0.1", self.target) else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED


@contextlib.contextmanager
def ssh_server(target, key, client_key, reject_auth=False):
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    listener.settimeout(0.2)
    stopped = threading.Event()
    transports = []

    def forward(channel):
        try:
            with socket.create_connection(("127.0.0.1", target), timeout=10) as remote:
                while not stopped.is_set():
                    ready, _, _ = select.select([channel, remote], [], [], 0.2)
                    for source in ready:
                        data = source.recv(65536)
                        if not data:
                            return
                        (remote if source is channel else channel).sendall(data)
        except (OSError, EOFError):
            pass
        finally:
            channel.close()

    def client(sock):
        transport = paramiko.Transport(sock)
        transports.append(transport)
        try:
            transport.add_server_key(key)
            transport.start_server(server=ForwardServer(target, client_key, reject_auth))
            while not stopped.is_set() and transport.is_active():
                channel = transport.accept(0.2)
                if channel:
                    threading.Thread(target=forward, args=(channel,), daemon=True).start()
        except (OSError, EOFError, paramiko.SSHException):
            pass
        finally:
            transport.close()

    def accept():
        while not stopped.is_set():
            try:
                sock, _ = listener.accept()
                threading.Thread(target=client, args=(sock,), daemon=True).start()
            except socket.timeout:
                pass
            except OSError:
                break

    thread = threading.Thread(target=accept, daemon=True)
    thread.start()
    try:
        yield listener.getsockname()[1], transports
    finally:
        stopped.set()
        listener.close()
        for transport in transports:
            transport.close()
        thread.join(timeout=2)


def ssh_request(port, action="", body=None, origin=None):
    url = f"http://127.0.0.1:{port}/__dsh-workspaces/ssh" + ("/" + action if action else "")
    request = urllib.request.Request(url, data=None if body is None else json.dumps(body).encode(), headers={"Content-Type":"application/json", "Origin":origin or f"http://127.0.0.1:{port}"})
    try:
        with urllib.request.urlopen(request, timeout=40) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        return error.code, json.load(error)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--workdir", type=pathlib.Path, required=True)
    args = parser.parse_args()
    work = args.workdir.resolve() / ("ssh-" + uuid.uuid4().hex)
    local, remote = work / "local", work / "remote"
    local.mkdir(parents=True)
    remote.mkdir()
    project = remote / "project with spaces"
    project.mkdir()
    (project / "marker.txt").write_text("remote-only-marker", encoding="utf-8")
    local_env = isolated_environment(local, local / "home")
    remote_env = isolated_environment(remote, remote / "home")
    key = paramiko.RSAKey.generate(2048)
    client_key = paramiko.RSAKey.generate(2048)
    identity = work / "fixture_key"
    client_key.write_private_key_file(str(identity))
    if sys.platform == "win32":
        subprocess.run(["icacls",str(identity),"/reset"],check=True,capture_output=True,creationflags=subprocess.CREATE_NO_WINDOW)
        subprocess.run(["icacls",str(identity),"/inheritance:r","/grant:r",getpass.getuser()+":R"],check=True,capture_output=True,creationflags=subprocess.CREATE_NO_WINDOW)
    else:
        os.chmod(identity,0o600)
    config_file, known_hosts = work / "ssh_config", work / "known_hosts"
    config_file.write_text(f'Host *\n  UserKnownHostsFile "{known_hosts.as_posix()}"\n  GlobalKnownHostsFile "{(work / "absent-global").as_posix()}"\n  IdentityFile "{identity.as_posix()}"\n  IdentitiesOnly yes\n', encoding="utf-8")
    with running_fixture_host(args.binary.resolve(), remote, remote_env, None, "ssh-remote") as remote_port:
        with ssh_server(remote_port, key, client_key) as (ssh_port, transports):
            known_hosts.write_text(f"[127.0.0.1]:{ssh_port} {key.get_name()} {key.get_base64()}\n", encoding="utf-8")
            config = {"id":str(uuid.uuid4()),"host":"127.0.0.1","port":ssh_port,"remotePort":remote_port,"user":"fixture","path":str(project),"configFile":str(config_file)}
            with running_fixture_host(args.binary.resolve(), local, local_env, None, "ssh-local") as local_port:
                assert ssh_request(local_port, "connect", config, "https://untrusted.invalid")[0] == 403
                assert ssh_request(local_port, "connect", {**config,"host":"-oProxyCommand=bad"})[0] == 400
                before = require_ok(rpc(local_port,"workspace.list",{},1),"local list")
                status, connected = ssh_request(local_port, "connect", config)
                if status != 200:
                    diagnostic=subprocess.run(['ssh','-v','-F',str(config_file),'-p',str(ssh_port),'-o','BatchMode=yes','-o','StrictHostKeyChecking=yes','-W',f'127.0.0.1:{remote_port}','fixture@127.0.0.1'],input=b'GET / HTTP/1.0\r\nConnection: close\r\n\r\n',capture_output=True,timeout=15,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
                    (work/'ssh-diagnostic.log').write_bytes(diagnostic.stderr)
                    print('SSH diagnostic:',diagnostic.stderr.decode('utf-8',errors='replace')[-6000:])
                assert status == 200, connected
                tunnel_port = urllib.parse.urlsplit(connected["url"]).port
                assert connected["execution"] == "remote-host"
                assert require_ok(rpc(local_port,"workspace.list",{},2),"local isolation") == before
                listed = require_ok(rpc(tunnel_port,"workspace.list",{},3),"remote list")["items"]
                workspace = next(item for item in listed if "project with spaces" in item["path"])
                session = require_ok(rpc(tunnel_port,"session.create",{"workspaceId":workspace["workspaceId"]},4),"remote session")["sessionId"]
                terminal = preview(tunnel_port,"terminal-action",body={"sessionId":session,"action":"open","name":"SSH acceptance"})["id"]
                command = "[IO.File]::WriteAllText((Join-Path (Get-Location) 'ssh-proof.txt'), 'ssh-remote-write')\r" if sys.platform == "win32" else "printf ssh-remote-write > ssh-proof.txt\r"
                preview(tunnel_port,"terminal-action",body={"sessionId":session,"action":"input","terminalId":terminal,"text":command})
                for _ in range(100):
                    if file_has_text(project / "ssh-proof.txt", "ssh-remote-write"):
                        break
                    time.sleep(0.1)
                assert file_has_text(project / "ssh-proof.txt", "ssh-remote-write")
                assert not (local / "ssh-proof.txt").exists()
                long_command="ping -n 30 127.0.0.1 | Out-Null\r" if sys.platform=="win32" else "sleep 30\r"
                preview(tunnel_port,"terminal-action",body={"sessionId":session,"action":"input","terminalId":terminal,"text":long_command})
                time.sleep(.5)
                preview(tunnel_port,"terminal-action",body={"sessionId":session,"action":"signal","terminalId":terminal,"signal":"SIGINT"})
                time.sleep(.5)
                recover="[IO.File]::WriteAllText((Join-Path (Get-Location) 'ssh-after-cancel.txt'), 'ssh-after-cancel')\r" if sys.platform=="win32" else "printf ssh-after-cancel > ssh-after-cancel.txt\r"
                preview(tunnel_port,"terminal-action",body={"sessionId":session,"action":"input","terminalId":terminal,"text":recover})
                for _ in range(100):
                    if file_has_text(project/'ssh-after-cancel.txt','ssh-after-cancel'):break
                    time.sleep(.1)
                assert file_has_text(project/'ssh-after-cancel.txt','ssh-after-cancel')
                preview(tunnel_port,"terminal-action",body={"sessionId":session,"action":"close","terminalId":terminal})
                assert ssh_request(local_port,"disconnect",{"id":config["id"]})[0] == 200
                assert ssh_request(local_port)[1]["items"][0]["state"] == "disconnected"
                assert ssh_request(local_port,"connect",config)[0] == 200
                for transport in transports:
                    transport.close()
                for _ in range(60):
                    if ssh_request(local_port)[1]["items"][0]["state"] == "disconnected":
                        break
                    time.sleep(0.2)
                assert ssh_request(local_port)[1]["items"][0]["state"] == "disconnected"
                assert ssh_request(local_port,"connect",config)[0] == 200
            with running_fixture_host(args.binary.resolve(), local, local_env, None, "ssh-restarted") as local_port:
                saved = ssh_request(local_port)[1]["items"][0]
                assert saved["state"] == "disconnected" and saved["connection"] == config
                known_hosts.write_text("",encoding="utf-8")
                assert ssh_request(local_port,"connect",config)[0] == 400, "unknown host key must fail closed"
                known_hosts.write_text(f"[127.0.0.1]:{ssh_port} {key.get_name()} {key.get_base64()}\n", encoding="utf-8")
                assert ssh_request(local_port,"connect",{**config,"path":str(remote / "missing")})[0] == 400
                with ssh_server(remote_port,key,client_key,reject_auth=True) as (denied_port,_):
                    with known_hosts.open('a',encoding='utf-8') as stream:
                        stream.write(f"[127.0.0.1]:{denied_port} {key.get_name()} {key.get_base64()}\n")
                    assert ssh_request(local_port,"connect",{**config,"port":denied_port})[0] == 400
                with socket.socket() as stalled:
                    stalled.bind(('127.0.0.1',0));stalled.listen()
                    with concurrent.futures.ThreadPoolExecutor() as pool:
                        waiting=pool.submit(ssh_request,local_port,'connect',{**config,'port':stalled.getsockname()[1]})
                        for _ in range(100):
                            if ssh_request(local_port)[1]['items'][0]['state']=='connecting':break
                            time.sleep(.05)
                        assert ssh_request(local_port,'disconnect',{'id':config['id']})[0]==200
                        assert waiting.result(timeout=8)[0]==400
                assert ssh_request(local_port,"remove",{"id":config["id"]})[0] == 200
                assert ssh_request(local_port)[1]["items"] == []
    profile=local/'home'/'ssh-workspaces.json'
    profile.write_text('{invalid',encoding='utf-8')
    with running_fixture_host(args.binary.resolve(),local,local_env,None,'ssh-invalid-config') as local_port:
        require_ok(rpc(local_port,'workspace.list',{},90),'local workspace with invalid SSH config')
        assert ssh_request(local_port)[1]['error']
        assert ssh_request(local_port,'connect',config)[0]==400
        assert profile.read_text(encoding='utf-8')=='{invalid'
    evidence = {"passed":True,"realOpenSshTransport":True,"remoteWorkspaceIsolation":True,"remoteTerminalWrite":True,"remoteTerminalCancellation":True,"disconnectReconnect":True,"connectionLoss":True,"restartRestoresDisconnectedProfile":True,"unknownHostKeyRejected":True,"missingRemotePathRejected":True,"csrfRejected":True,"publicKeyAuthentication":True,"authenticationRejection":True,"connectCancellation":True,"invalidSshConfigIsolated":True,"macosLinuxDesktopAcceptance":False}
    (work / "result.json").write_text(json.dumps(evidence,indent=2),encoding="utf-8")
    print(json.dumps({**evidence,"work":str(work)}))


if __name__ == "__main__":
    main()
