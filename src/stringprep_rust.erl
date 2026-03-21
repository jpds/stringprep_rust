%%%----------------------------------------------------------------------
%%% File    : stringprep_rust.erl
%%% Purpose : Rust NIF replacement for stringprep with LRU cache
%%% Created : 2026-03-21
%%%
%%% Copyright (C) 2026 Licensed under the Apache License, Version 2.0
%%%----------------------------------------------------------------------

-module(stringprep_rust).
-on_load(init/0).

-export([tolower/1, tolower_nofilter/1, nameprep/1,
         nodeprep/1, resourceprep/1,
         cache_size/0, cache_stats/0]).

init() ->
    SOPath = filename:join([code:priv_dir(stringprep_rust),
                            "crates", "stringprep_rust", "stringprep_rust"]),
    erlang:load_nif(SOPath, 0).

%% Public API — accepts iodata(), converts to binary before NIF call.
%% This matches the original stringprep C NIF behavior where
%% enif_inspect_iolist_as_binary flattens iolists.

-spec nodeprep(iodata()) -> binary() | error.
nodeprep(String) ->
    nodeprep_nif(iolist_to_binary(String)).

-spec nameprep(iodata()) -> binary() | error.
nameprep(String) ->
    nameprep_nif(iolist_to_binary(String)).

-spec resourceprep(iodata()) -> binary() | error.
resourceprep(String) ->
    resourceprep_nif(iolist_to_binary(String)).

-spec tolower(iodata()) -> binary() | error.
tolower(String) ->
    tolower_nif(iolist_to_binary(String)).

-spec tolower_nofilter(iodata()) -> binary() | error.
tolower_nofilter(String) ->
    tolower_nofilter_nif(iolist_to_binary(String)).

%% Cache management
-spec cache_size() -> non_neg_integer().
cache_size() ->
    erlang:nif_error(nif_not_loaded).

-spec cache_stats() -> {non_neg_integer(), non_neg_integer()}.
cache_stats() ->
    erlang:nif_error(nif_not_loaded).

%% NIF stubs (binary-only, called after iolist_to_binary)
nodeprep_nif(_Binary) -> erlang:nif_error(nif_not_loaded).
nameprep_nif(_Binary) -> erlang:nif_error(nif_not_loaded).
resourceprep_nif(_Binary) -> erlang:nif_error(nif_not_loaded).
tolower_nif(_Binary) -> erlang:nif_error(nif_not_loaded).
tolower_nofilter_nif(_Binary) -> erlang:nif_error(nif_not_loaded).
