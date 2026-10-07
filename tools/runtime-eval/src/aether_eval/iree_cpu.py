"""Real IREE Python API adapter, LLVM CPU backend and shared local-task device.

This does not implement an IREE HAL driver or connect IREE to the Aether kernel.
"""
import numpy as np
import iree.compiler as compiler
import iree.runtime as runtime


def source(job):
    if job['operation'] == 'normalize':
        h, w = job['shape']
        t = f'tensor<{h}x{w}xf32>'
        body = f'''func.func @main(%a: {t}) -> {t} {{
          %scale = arith.constant dense<0.00392156862745098> : {t}
          %r = arith.mulf %a, %scale : {t}
          return %r : {t}
        }}'''
    else:
        m, k, n = job['shape']
        a, b, c = f'tensor<{m}x{k}xf32>', f'tensor<{k}x{n}xf32>', f'tensor<{m}x{n}xf32>'
        body = f'''func.func @main(%a: {a}, %b: {b}) -> {c} {{
          %zero = arith.constant 0.0 : f32
          %empty = tensor.empty() : {c}
          %init = linalg.fill ins(%zero : f32) outs(%empty : {c}) -> {c}
          %r = linalg.matmul ins(%a, %b : {a}, {b}) outs(%init : {c}) -> {c}
          return %r : {c}
        }}'''
    return 'module {\n' + body + '\n}\n'


def prepare(job, seed, directory):
    name = job['name']
    text = source(job)
    (directory / f'{name}.mlir').write_text(text, encoding='utf-8')
    binary = compiler.compile_str(text, target_backends=['llvm-cpu'],
                                  extra_args=['--iree-llvmcpu-target-cpu=generic'])
    (directory / f'{name}.vmfb').write_bytes(binary)
    rng = np.random.Generator(np.random.PCG64(seed))
    if job['operation'] == 'normalize':
        a = rng.integers(0, 256, size=job['shape']).astype(np.float32)
        arrays = {'a': a, 'expected': a / np.float32(255)}
    else:
        m, k, n = job['shape']
        a = rng.uniform(-1, 1, (m, k)).astype(np.float32)
        b = rng.uniform(-1, 1, (k, n)).astype(np.float32)
        arrays = {'a': a, 'b': b, 'expected': a @ b}
    np.savez(directory / f'{name}.npz', **arrays)


class Session:
    def __init__(self, job, directory, config):
        self.context = runtime.SystemContext(config=config)
        module = runtime.VmModule.copy_buffer(config.vm_instance, (directory / f'{job["name"]}.vmfb').read_bytes())
        self.context.add_vm_module(module)
        self.function = self.context.modules.module.main
        with np.load(directory / f'{job["name"]}.npz', allow_pickle=False) as arrays:
            self.inputs = [arrays['a'].copy()]
            if job['operation'] == 'matmul':
                self.inputs.append(arrays['b'].copy())
            self.expected = arrays['expected'].copy()

    def invoke(self):
        # Materializing to host waits for completion; enqueue time alone is not latency.
        return self.function(*self.inputs).to_host().copy()

    def correct(self, output):
        return output.shape == self.expected.shape and bool(np.allclose(output, self.expected, rtol=1e-4, atol=1e-4))

    def reject_bad_shape(self):
        args = [self.inputs[0][:-1], *self.inputs[1:]]
        try:
            self.function(*args).to_host()
        except ValueError as exc:
            message = str(exc)
            return {'passed': 'INVALID_ARGUMENT' in message and 'shape' in message.lower(), 'diagnostic': message}
        return {'passed': False, 'diagnostic': 'Runtime accepted an invalid input shape'}


def make_config():
    return runtime.Config('local-task')
