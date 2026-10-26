"""Builds the tiny test model in `tiny/` (not a real GLiNER model).

It has GLiNER's input/output signature and marks every word whose first
sub-token is `anna` as a one-word span of the first label, so the Rust side
can be tested end to end (tokenizer, words_mask, spans, decoding) without
downloading a real model. Run with: python make_tiny.py (needs onnx, tokenizers).
"""
import json, os
import onnx
from onnx import TensorProto, helper as h
from tokenizers import Tokenizer, models, pre_tokenizers, processors

here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "tiny")
os.makedirs(here, exist_ok=True)

vocab = {t: i for i, t in enumerate(
    ["[PAD]", "[UNK]", "[CLS]", "[SEP]", "<<ENT>>", "<<SEP>>", "person", "health", "condition",
     "hallo", "anna", "weber", "ist", "krank", ",", "."])}
tok = Tokenizer(models.WordLevel(vocab=vocab, unk_token="[UNK]"))
tok.normalizer = None
tok.pre_tokenizer = pre_tokenizers.Whitespace()
tok.post_processor = processors.TemplateProcessing(
    single="[CLS] $A [SEP]", special_tokens=[("[CLS]", vocab["[CLS]"]), ("[SEP]", vocab["[SEP]"])])
from tokenizers import normalizers
tok.normalizer = normalizers.Lowercase()
tok.add_special_tokens(["<<ENT>>", "<<SEP>>"])
tok.save(os.path.join(here, "tokenizer.json"))

json.dump({"max_width": 12, "ent_token": "<<ENT>>", "sep_token": "<<SEP>>", "span_mode": "markerV0"},
          open(os.path.join(here, "gliner_config.json"), "w"), indent=1)

W, ANNA, ENT = 12, vocab["anna"], vocab["<<ENT>>"]
i64 = lambda name, v: h.make_tensor(name, TensorProto.INT64, [len(v)] if isinstance(v, list) else [], v if isinstance(v, list) else [v])
nodes = [
    # L = text_lengths[0,0]; words k = 1..L
    h.make_node("Reshape", ["text_lengths", "shape_scalar"], ["L"]),
    h.make_node("Add", ["L", "one"], ["L1"]),
    h.make_node("Range", ["one", "L1", "one"], ["ks"]),                       # [L]
    h.make_node("Unsqueeze", ["ks", "ax0"], ["ks_row"]),                      # [1, L]
    h.make_node("Reshape", ["words_mask", "col"], ["wm_col"]),                # [T, 1]
    h.make_node("Equal", ["wm_col", "ks_row"], ["M_b"]),                       # [T, L]
    h.make_node("Cast", ["M_b"], ["M"], to=TensorProto.FLOAT),
    h.make_node("Equal", ["input_ids", "anna"], ["is_anna_b"]),               # [1, T]
    h.make_node("Cast", ["is_anna_b"], ["is_anna"], to=TensorProto.FLOAT),
    h.make_node("MatMul", ["is_anna", "M"], ["word_hit"]),                    # [1, L]
    # number of labels C = count of <<ENT>>
    h.make_node("Equal", ["input_ids", "ent"], ["is_ent_b"]),
    h.make_node("Cast", ["is_ent_b"], ["is_ent"], to=TensorProto.INT64),
    h.make_node("ReduceSum", ["is_ent"], ["C"], keepdims=0),
    h.make_node("Range", ["zero", "C", "one"], ["cs"]),                       # [C]
    h.make_node("Equal", ["cs", "zero"], ["c0_b"]),
    h.make_node("Cast", ["c0_b"], ["c0"], to=TensorProto.FLOAT),
    h.make_node("Unsqueeze", ["c0", "ax0"], ["c0_row"]),                      # [1, C]
    h.make_node("MatMul", ["j0_col", "c0_row"], ["P"]),                       # [W, C]
    h.make_node("Reshape", ["word_hit", "hit_shape"], ["hit4"]),              # [1, L, 1, 1]
    h.make_node("Mul", ["hit4", "P"], ["grid"]),                              # [1, L, W, C]
    h.make_node("Mul", ["grid", "twenty"], ["g20"]),
    h.make_node("Sub", ["g20", "ten"], ["logits"]),
    # keep the other inputs in the graph
    h.make_node("Identity", ["attention_mask"], ["_am"]),
]
inits = [
    i64("shape_scalar", [1]), i64("one", 1), i64("zero", 0), i64("ax0", [0]), i64("col", [-1, 1]),
    i64("anna", ANNA), i64("ent", ENT), i64("hit_shape", [1, -1, 1, 1]),
    h.make_tensor("j0_col", TensorProto.FLOAT, [W, 1], [1.0] + [0.0] * (W - 1)),
    h.make_tensor("twenty", TensorProto.FLOAT, [], [20.0]), h.make_tensor("ten", TensorProto.FLOAT, [], [10.0]),
]
inp = lambda n, t, s: h.make_tensor_value_info(n, t, s)
graph = h.make_graph(nodes, "tiny_gliner", [
    inp("input_ids", TensorProto.INT64, [1, "T"]), inp("attention_mask", TensorProto.INT64, [1, "T"]),
    inp("words_mask", TensorProto.INT64, [1, "T"]), inp("text_lengths", TensorProto.INT64, [1, 1]),
    inp("span_idx", TensorProto.INT64, [1, "S", 2]), inp("span_mask", TensorProto.BOOL, [1, "S"]),
], [h.make_tensor_value_info("logits", TensorProto.FLOAT, [1, "L", W, "C"])], inits)
model = h.make_model(graph, opset_imports=[h.make_opsetid("", 13)])
model.ir_version = 8
onnx.checker.check_model(model)
onnx.save(model, os.path.join(here, "model.onnx"))
print("ok")
