"""Fetch NVIDIA CUDA development headers only; no driver/GPU/FFmpeg install."""
import hashlib, io, pathlib, urllib.request, zipfile

url = 'https://files.pythonhosted.org/packages/bc/46/a92db19b8309581092a3add7e6fceb4c301a3fd233969856a8cbf042cd3c/nvidia_cuda_runtime_cu12-12.9.79-py3-none-manylinux2014_x86_64.manylinux_2_17_x86_64.whl'
data = urllib.request.urlopen(url, timeout=60).read()
assert hashlib.sha256(data).hexdigest() == '25bba2dfb01d48a9b59ca474a1ac43c6ebf7011f1b0b8cc44f54eb6ac48a96c3'
target = pathlib.Path('/opt/uvms-nvidia-sdk')
with zipfile.ZipFile(io.BytesIO(data)) as archive:
    for item in archive.infolist():
        if item.is_dir():
            continue
        if item.filename.startswith('nvidia/cuda_runtime/include/') or 'licenses/' in item.filename:
            relative = pathlib.PurePosixPath(item.filename)
            assert '..' not in relative.parts
            dest = target / relative
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_bytes(archive.read(item))
print('CUDA_INCLUDE_DIR=' + str(target / 'nvidia/cuda_runtime/include'))
