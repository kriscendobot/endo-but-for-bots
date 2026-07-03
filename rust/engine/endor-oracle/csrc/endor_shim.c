/*
 * endor-oracle shim.
 *
 * Compiled alongside the C-XS sources (the c/moddable submodule pin
 * the endor daemon builds today), with the same feature defines as
 * the xsnap crate, so it can use the internal xsAll.h API directly
 * the way fx_eval() in xsGlobal.c does.
 *
 * It gives the differential harness two things the public xsnap
 * Machine API does not expose:
 *
 *   1. the exact XS bytecode the C-XS compiler emits for a program
 *      (so the Rust interpreter can execute the identical byte
 *      stream during stages 1 through 4, before the Rust compiler
 *      lands in stage 5), and
 *
 *   2. a run-only computron count: metering is reset to zero after
 *      parse and read after run, so parse metering
 *      (XS_PARSE_CODE_METERING) does not contaminate the interpreter
 *      parity number (design, "Stages 1 through 4 keep the oracle
 *      compiler in the loop ... a computron divergence always has
 *      exactly one suspect").
 *
 * This is the only crate in rust/engine that touches unsafe / FFI;
 * it is dev-and-CI only and never linked into a shipped engine.
 */

#include "xsAll.h"
#include <stdlib.h>
#include <string.h>

#define ENDOR_RESULT_MAX 1024
#define ENDOR_ERROR_MAX 256

typedef struct {
	txS1 *code;   /* malloc'd copy of the program bytecode; caller frees */
	txU4 code_size;
	txS1 *symbols; /* malloc'd copy of the symbols atom; caller frees */
	txU4 symbols_size;
	txU4 computrons; /* meterIndex >> 16 over the run only */
	txU4 meter_raw; /* raw meterIndex over the run only (diagnostic) */
	txU4 ok;         /* 1 = completed normally, 0 = threw / parse error */
	char result[ENDOR_RESULT_MAX]; /* completion value coerced to String() */
	char error[ENDOR_ERROR_MAX];   /* message when ok == 0 */
} EndorOracleResult;

static int gEndorClusterReady = 0;

/* Mirrors DEFAULT_CREATION in rust/endo/xsnap/src/lib.rs. */
static txCreation gEndorCreation = {
	128 * 1024, /* initialChunkSize */
	64 * 1024,  /* incrementalChunkSize */
	8192,       /* initialHeapCount */
	4096,       /* incrementalHeapCount */
	4096,       /* stackCount */
	2048,       /* initialKeyCount */
	512,        /* incrementalKeyCount */
	127,        /* nameModulo */
	127,        /* symbolModulo */
	8192 * 1024,/* parserBufferSize */
	1993,       /* parserTableModulo */
	0,          /* staticSize */
	0,          /* nativeStackSize */
};

/*
 * Run `source` as a program eval on a fresh machine.
 * Returns 0 on success (machine created and result populated),
 * negative on a machine-level failure.  A thrown JS exception or a
 * syntax error is a normal outcome reported through out->ok == 0.
 */
int endor_oracle_run(const char *source, txU4 sourceLen, EndorOracleResult *out)
{
	txMachine *the;
	memset(out, 0, sizeof(*out));

	if (!gEndorClusterReady) {
		fxInitializeSharedCluster(C_NULL);
		gEndorClusterReady = 1;
	}

	the = fxCreateMachine(&gEndorCreation, "endor-oracle", C_NULL, 0);
	if (!the)
		return -1;

	the = fxBeginHost(the);
	{
		mxTry(the) {
			txStringCStream stream;
			txScript *script;
			txSlot *module;
			txSlot *realm;
			txSlot *result;

			stream.buffer = (txString)source;
			stream.offset = 0;
			stream.size = (txSize)sourceLen;

			/* Compile (parse+code). Parse metering is discarded below. */
			script = fxParseScript(the, &stream, fxStringCGetter,
				mxProgramFlag | mxEvalFlag);

			/* Capture the emitted bytecode before running. */
			out->code_size = (txU4)script->codeSize;
			if (script->codeSize > 0) {
				out->code = (txS1 *)malloc(script->codeSize);
				if (out->code)
					memcpy(out->code, script->codeBuffer, script->codeSize);
			}
			if (script->symbolsBuffer && script->symbolsSize > 0) {
				out->symbols_size = (txU4)script->symbolsSize;
				out->symbols = (txS1 *)malloc(script->symbolsSize);
				if (out->symbols)
					memcpy(out->symbols, script->symbolsBuffer, script->symbolsSize);
			}

			/* The initial program instance carries the realm. */
			module = mxProgram.value.reference;
			realm = mxModuleInstanceInternal(module)->value.module.realm;

			/* Measure the run only. */
			the->meterIndex = 0;
			fxRunScript(the, script, mxRealmGlobal(realm), C_NULL,
				mxRealmClosures(realm)->value.reference, C_NULL, module);
			out->computrons = the->meterIndex >> 16;
			out->meter_raw = (txU4)the->meterIndex;

			/* fxRunScript leaves the completion value on the stack top. */
			result = the->stack;
			fxToString(the, result);
			{
				txString s = result->value.string;
				if (s) {
					strncpy(out->result, s, ENDOR_RESULT_MAX - 1);
					out->result[ENDOR_RESULT_MAX - 1] = 0;
				}
			}
			mxPop();
			out->ok = 1;
		}
		mxCatch(the) {
			out->ok = 0;
			/* Record the run-only computron count reached at the point of
			 * an uncaught throw, exactly as the normal-completion path
			 * does. meterIndex was reset to 0 immediately before
			 * fxRunScript (above) and the longjmp out of the run
			 * preserves it, so this is the run-phase count when the throw
			 * originated in execution. (A parse-phase failure — before the
			 * reset — leaves a parse-metering value here, but such a run
			 * yields empty/undecodable bytecode on the endor side and so
			 * is never a bit-exact BothAbort regardless.) This is what lets
			 * the dual-run harness compare computrons on the abort path,
			 * not only the completion path (stage-2a review observation 3). */
			out->computrons = the->meterIndex >> 16;
			out->meter_raw = (txU4)the->meterIndex;
			/* mxException holds the thrown value; stringify best-effort. */
			if (mxException.kind != XS_UNDEFINED_KIND) {
				mxPush(mxException);
				fxToString(the, the->stack);
				if (the->stack->value.string) {
					strncpy(out->error, the->stack->value.string, ENDOR_ERROR_MAX - 1);
					out->error[ENDOR_ERROR_MAX - 1] = 0;
				}
				mxPop();
			}
		}
	}
	fxEndHost(the);
	fxDeleteMachine(the);
	return 0;
}

void endor_oracle_free(EndorOracleResult *out)
{
	if (out->code) {
		free(out->code);
		out->code = C_NULL;
	}
	if (out->symbols) {
		free(out->symbols);
		out->symbols = C_NULL;
	}
}
