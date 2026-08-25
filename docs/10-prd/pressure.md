## Context Pressure

Each Profile sets a maximum complete GitHub Context byte budget and a soft
ratio, default 80 percent. The byte budget is explicit because providers do not
expose one reliable tokenizer/window contract across models. Soft pressure asks
the Agent or Human to update descriptions, shorten Agent-owned comments, or
fold obsolete discussion. Exceeding the hard budget, or failing to paginate a
required connection completely, blocks the turn and updates an Operational
Status Comment. Braid never silently truncates or summarizes canonical Context.
