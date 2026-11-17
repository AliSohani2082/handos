--!strict
-- Spike stand-in for the handOs Luau standard library: process helpers built on the host's
-- sleep / wait / spawn / emit, plus pure-Luau signal and scope utilities.

local std = {}

export type Cleanup = () -> ()

export type Scope = {
	cleanups: { Cleanup },
	closed: boolean,
	defer: (self: Scope, fn: Cleanup) -> (),
	close: (self: Scope) -> (),
}

local Scope = {}
Scope.__index = Scope

function Scope.defer(self: Scope, fn: Cleanup)
	assert(not self.closed, "scope already closed")
	table.insert(self.cleanups, fn)
end

function Scope.close(self: Scope)
	if self.closed then
		return
	end
	self.closed = true
	for i = #self.cleanups, 1, -1 do
		local ok, err = pcall(self.cleanups[i])
		if not ok then
			log("cleanup failed: " .. tostring(err))
		end
	end
	table.clear(self.cleanups)
end

-- Runs fn(scope); the scope's cleanups run however fn ends.
function std.with_scope<T...>(fn: (Scope) -> T...): T...
	local scope = setmetatable({ cleanups = {}, closed = false }, Scope) :: any
	local result = table.pack(pcall(fn, scope))
	scope:close()
	if not result[1] then
		error(result[2], 0)
	end
	return table.unpack(result, 2, result.n)
end

-- Waits for `event`, or fails after `seconds`.
function std.wait_for(event: string, seconds: number): string
	local token = "timeout:" .. event .. ":" .. tostring(now())
	local done = false
	spawn(function()
		sleep(seconds)
		if not done then
			emit(event, token)
		end
	end, "timeout")
	local value = wait(event)
	done = true
	if value == token then
		error("timed out waiting for " .. event, 2)
	end
	return value
end

function std.retry<T>(attempts: number, fn: () -> T): T
	local last: any = nil
	for i = 1, attempts do
		local ok, value = pcall(fn)
		if ok then
			return value
		end
		last = value
		sleep(0.01 * i)
	end
	error(last, 0)
end

-- Signals: small expression trees the host would evaluate at display rate.
export type Signal = { kind: string, args: { any } }

local function node(kind: string, ...: any): Signal
	return { kind = kind, args = { ... } }
end

std.signal = {
	landmark = function(hand: string, index: number): Signal
		return node("landmark", hand, index)
	end,
	lerp = function(a: Signal, b: Signal, t: number): Signal
		return node("lerp", a, b, t)
	end,
	spring = function(target: Signal, stiffness: number?, damping: number?): Signal
		return node("spring", target, stiffness or 300, damping or 30)
	end,
	offset = function(base: Signal, dx: number, dy: number): Signal
		return node("offset", base, dx, dy)
	end,
}

-- Triggers: declarations the host would evaluate at tracking rate.
export type Trigger = { kind: string, args: { any }, hold: number?, cooldown: number? }

std.trigger = {
	pinch = function(hand: string, fingers: { string }?): Trigger
		return { kind = "pinch", args = { hand, fingers or { "thumb", "index" } } }
	end,
	near = function(a: Signal, b: Signal, distance: number): Trigger
		return { kind = "near", args = { a, b, distance } }
	end,
	all = function(...: Trigger): Trigger
		return { kind = "all", args = { ... } }
	end,
	any = function(...: Trigger): Trigger
		return { kind = "any", args = { ... } }
	end,
	hold = function(t: Trigger, seconds: number): Trigger
		local copy = table.clone(t)
		copy.hold = seconds
		return copy
	end,
}

function std.map<T, U>(items: { T }, fn: (T) -> U): { U }
	local out = table.create(#items)
	for i, v in items do
		out[i] = fn(v)
	end
	return out
end

function std.filter<T>(items: { T }, fn: (T) -> boolean): { T }
	local out = {}
	for _, v in items do
		if fn(v) then
			table.insert(out, v)
		end
	end
	return out
end

return std
