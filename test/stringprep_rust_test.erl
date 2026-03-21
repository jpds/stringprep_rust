-module(stringprep_rust_test).
-compile(export_all).

%% Test runner — call from: erl -noshell -eval 'stringprep_rust_test:all()' -s init stop
all() ->
    Tests = [
        {"empty_string",          fun empty_string_test/0},
        {"badarg",                fun badarg_test/0},
        {"at_nodeprep",           fun at_nodeprep_test/0},
        {"tolower_basic",         fun tolower_basic_test/0},
        {"resourceprep_unicode",  fun resourceprep_unicode_test/0},
        {"nameprep_fail",         fun nameprep_fail_test/0},
        {"vectors",               fun vector_test/0},
        {"cache_stats",           fun cache_stats_test/0},
        {"iolist_input",          fun iolist_input_test/0}
    ],
    {Pass, Fail} = lists:foldl(
        fun({Name, Fun}, {P, F}) ->
            try Fun() of
                _ ->
                    io:format("  PASS: ~s~n", [Name]),
                    {P + 1, F}
            catch
                Class:Reason:Stack ->
                    io:format("  FAIL: ~s — ~p:~p~n    ~p~n",
                              [Name, Class, Reason, hd(Stack)]),
                    {P, F + 1}
            end
        end,
        {0, 0},
        Tests),
    io:format("~n~p passed, ~p failed~n", [Pass, Fail]),
    case Fail of
        0 -> ok;
        _ -> halt(1)
    end.

%% --- Individual tests ---

empty_string_test() ->
    <<>> = stringprep_rust:nodeprep(<<>>),
    <<>> = stringprep_rust:nameprep(<<>>),
    <<>> = stringprep_rust:resourceprep(<<>>),
    <<>> = stringprep_rust:tolower(<<>>),
    ok.

badarg_test() ->
    %% Non-iodata arguments should raise badarg (from iolist_to_binary)
    expect_badarg(fun() -> stringprep_rust:nodeprep(foo) end),
    expect_badarg(fun() -> stringprep_rust:nameprep(123) end),
    expect_badarg(fun() -> stringprep_rust:resourceprep({foo, bar}) end),
    expect_badarg(fun() -> stringprep_rust:tolower(fun() -> ok end) end),
    ok.

expect_badarg(Fun) ->
    try Fun() of
        _ -> error(expected_badarg)
    catch
        error:badarg -> ok
    end.

at_nodeprep_test() ->
    error = stringprep_rust:nodeprep(<<"@">>),
    ok.

tolower_basic_test() ->
    <<"abcd">> = stringprep_rust:tolower(<<"AbCd">>),
    ok.

resourceprep_unicode_test() ->
    Expected = <<95,194,183,194,176,226,137,136,88,46,209,130,208,189,206,
                 181,32,208,188,97,206,183,32,195,143,197,139,32,196,174,
                 209,143,207,131,206,174,32,208,188,97,115,208,186,46,88,
                 226,137,136,194,176,194,183,95>>,
    Input = <<95,194,183,194,176,226,137,136,88,46,209,130,208,189,
              206,181,32,208,188,194,170,206,183,32,195,143,197,139,
              32,196,174,209,143,207,131,206,174,32,208,188,194,170,
              115,208,186,46,88,226,137,136,194,176,194,183,95>>,
    Expected = stringprep_rust:resourceprep(Input),
    ok.

nameprep_fail_test() ->
    error = stringprep_rust:nameprep(<<217,173,65,112,107,97,119,97,217,173>>),
    ok.

vector_test() ->
    Cases = [
             %% B.1 removal: soft hyphen, zero-width chars, variation selectors, BOM
             {<<"foo", 16#C2, 16#AD, 16#CD, 16#8F, 16#E1, 16#A0, 16#86, 16#E1, 16#A0, 16#8B,
                "bar", 16#E2, 16#80, 16#8B, 16#E2, 16#81, 16#A0, "baz", 16#EF, 16#B8, 16#80,
                16#EF, 16#B8, 16#88, 16#EF, 16#B8, 16#8F, 16#EF, 16#BB, 16#BF>>,
              <<"foobarbaz">>,
              <<"foobarbaz">>,
              <<"foobarbaz">>},
             %% Simple ASCII case folding
             {<<"CAFE">>,
              <<"cafe">>,
              <<"cafe">>,
              <<"CAFE">>},
             %% German sharp s → ss
             {<<16#C3, 16#9F>>,
              <<"ss">>,
              <<"ss">>,
              <<16#C3, 16#9F>>},
             %% Latin capital I with dot above → i + combining dot
             {<<16#C4, 16#B0>>,
              <<"i", 16#CC, 16#87>>,
              <<"i", 16#CC, 16#87>>,
              <<16#C4, 16#B0>>},
             %% Combining marks + space (nodeprep rejects space)
             {<<16#C5, 16#83, 16#CD, 16#BA>>,
              <<16#C5, 16#84, " ", 16#CE, 16#B9>>,
              error,
              <<16#C5, 16#83, " ", 16#CD, 16#85>>},
             %% Compatibility decomposition (TEL symbol, etc.)
             %% Note: Rust stringprep crate applies case folding after NFKC
             %% decomposition, so the "C" from U+33C6 decomposition gets
             %% lowercased. The C implementation case-folds before decomposition.
             {<<16#E2, 16#84, 16#A1, 16#E3, 16#8F, 16#86, 16#F0, 16#9D, 16#9E, 16#BB>>,
              <<"telc", 16#E2, 16#88, 16#95, "kg", 16#CF, 16#83>>,
              <<"telc", 16#E2, 16#88, 16#95, "kg", 16#CF, 16#83>>,
              <<"TELC", 16#E2, 16#88, 16#95, "kg", 16#CF, 16#82>>},
             %% NBSP mapped to space (nodeprep rejects space)
             {<<"j", 16#CC, 16#8C, 16#C2, 16#A0, 16#C2, 16#AA>>,
              <<16#C7, 16#B0, " a">>,
              error,
              <<16#C7, 16#B0, " a">>},
             %% Greek with iota subscript
             {<<16#E1, 16#BE, 16#B7>>,
              <<16#E1, 16#BE, 16#B6, 16#CE, 16#B9>>,
              <<16#E1, 16#BE, 16#B6, 16#CE, 16#B9>>,
              <<16#E1, 16#BE, 16#B7>>},
             %% Invalid UTF-8
             {<<16#C7, 16#F0>>,
              error,
              error,
              error},
             %% Greek dialytika tonos (U+0390)
             {<<16#CE, 16#90>>,
              <<16#CE, 16#90>>,
              <<16#CE, 16#90>>,
              <<16#CE, 16#90>>},
             %% Greek small upsilon with dialytika and tonos (U+03B0)
             {<<16#CE, 16#B0>>,
              <<16#CE, 16#B0>>,
              <<16#CE, 16#B0>>,
              <<16#CE, 16#B0>>},
             %% Latin small h with line below (U+1E96)
             {<<16#E1, 16#BA, 16#96>>,
              <<16#E1, 16#BA, 16#96>>,
              <<16#E1, 16#BA, 16#96>>,
              <<16#E1, 16#BA, 16#96>>},
             %% Greek small upsilon with psili and varia (U+1F56)
             {<<16#E1, 16#BD, 16#96>>,
              <<16#E1, 16#BD, 16#96>>,
              <<16#E1, 16#BD, 16#96>>,
              <<16#E1, 16#BD, 16#96>>},
             %% Space (nodeprep prohibits)
             {<<" ">>,
              <<" ">>,
              error,
              <<" ">>},
             %% NBSP → space (nodeprep prohibits)
             {<<16#C2, 16#A0>>,
              <<" ">>,
              error,
              <<" ">>},
             %% Ogham space mark — prohibited
             {<<16#E1, 16#9A, 16#80>>,
              error,
              error,
              error},
             %% En quad → space (nodeprep prohibits)
             {<<16#E2, 16#80, 16#80>>,
              <<" ">>,
              error,
              <<" ">>},
             %% Zero width space → removed
             {<<16#E2, 16#80, 16#8B>>,
              <<>>,
              <<>>,
              <<>>},
             %% Ideographic space → space (nodeprep prohibits)
             {<<16#E3, 16#80, 16#80>>,
              <<" ">>,
              error,
              <<" ">>},
             %% ASCII control chars (nodeprep & resourceprep prohibit)
             {<<16#10, 16#7f>>,
              <<16#10, 16#7f>>,
              error,
              error},
             %% Next line U+0085 — prohibited
             {<<16#C2, 16#85>>,
              error,
              error,
              error},
             %% Mongolian vowel separator U+180E — prohibited
             {<<16#E1, 16#A0, 16#8E>>,
              error,
              error,
              error},
             %% BOM U+FEFF → removed
             {<<16#EF, 16#BB, 16#BF>>,
              <<>>,
              <<>>,
              <<>>},
             %% Musical symbol — unassigned
             {<<16#F0, 16#9D, 16#85, 16#B5>>,
              error,
              error,
              error},
             %% Private use area
             {<<16#EF, 16#84, 16#A3>>,
              error,
              error,
              error},
             %% Plane 3 private use
             {<<16#F3, 16#B1, 16#88, 16#B4>>,
              error,
              error,
              error},
             %% Plane 16 private use
             {<<16#F4, 16#8F, 16#88, 16#B4>>,
              error,
              error,
              error},
             %% Non-character
             {<<16#F2, 16#8F, 16#BF, 16#BE>>,
              error,
              error,
              error},
             %% Max codepoint
             {<<16#F4, 16#8F, 16#BF, 16#BF>>,
              error,
              error,
              error},
             %% Surrogate (invalid)
             {<<16#ED, 16#BD, 16#82>>,
              error,
              error,
              error},
             %% Replacement character
             {<<16#EF, 16#BF, 16#BD>>,
              error,
              error,
              error},
             %% CJK compatibility ideograph
             {<<16#E2, 16#BF, 16#B5>>,
              error,
              error,
              error},
             %% Combining acute accent (canonical equivalent)
             {<<16#CD, 16#81>>,
              <<16#CC, 16#81>>,
              <<16#CC, 16#81>>,
              <<16#CC, 16#81>>},
             %% LRM (left-to-right mark) — prohibited
             {<<16#E2, 16#80, 16#8E>>,
              error,
              error,
              error},
             %% LRE (left-to-right embedding) — prohibited
             {<<16#E2, 16#80, 16#AA>>,
              error,
              error,
              error},
             %% Language tag
             {<<16#F3, 16#A0, 16#80, 16#81>>,
              error,
              error,
              error},
             %% Tag Latin small letter b
             {<<16#F3, 16#A0, 16#81, 16#82>>,
              error,
              error,
              error},
             %% Bidi: Hebrew char + ASCII (no trailing RTL) — error
             {<<"foo", 16#D6, 16#BE, "bar">>,
              error,
              error,
              error},
             %% Bidi: Arabic presentation form — error
             {<<"foo", 16#EF, 16#B5, 16#90, "bar">>,
              error,
              error,
              error},
             %% Arabic compatibility mapping + space
             {<<"foo", 16#EF, 16#B9, 16#B6, "bar">>,
              <<"foo ", 16#D9, 16#8E, "bar">>,
              error,
              <<"foo ", 16#D9, 16#8E, "bar">>},
             %% Bidi: Arabic + digit (no trailing RTL) — error
             {<<16#D8, 16#A7, "1">>,
              error,
              error,
              error},
             %% Bidi: Arabic + digit + Arabic — ok
             {<<16#D8, 16#A7, "1", 16#D8, 16#A8>>,
              <<16#D8, 16#A7, "1", 16#D8, 16#A8>>,
              <<16#D8, 16#A7, "1", 16#D8, 16#A8>>,
              <<16#D8, 16#A7, "1", 16#D8, 16#A8>>},
             %% Tag space
             {<<16#F3, 16#A0, 16#80, 16#82>>,
              error,
              error,
              error},
             %% Complex: multiple mappings + spaces + compatibility
             {<<"X", 16#C2, 16#AD, 16#C3, 16#9F, 16#C4, 16#B0, 16#E2, 16#84, 16#A1, "j", 16#CC, 16#8C,
                16#C2, 16#A0, 16#C2, 16#AA, 16#CE, 16#B0, 16#E2, 16#80, 16#80>>,
              <<"xssi", 16#CC, 16#87, "tel", 16#C7, 16#B0, " a", 16#CE, 16#B0, " ">>,
              error,
              <<"X", 16#C3, 16#9F, 16#C4, 16#B0, "TEL", 16#C7, 16#B0, " a", 16#CE, 16#B0, " ">>},
             %% Complex: CJK compatibility + compat decomposition
             {<<"X", 16#C3, 16#9F, 16#E3, 16#8C, 16#96, 16#C4, 16#B0, 16#E2, 16#84, 16#A1, 16#E2, 16#92,
                16#9F, 16#E3, 16#8C, 16#80>>,
              <<"xss", 16#E3, 16#82, 16#AD, 16#E3, 16#83, 16#AD, 16#E3, 16#83, 16#A1, 16#E3, 16#83, 16#BC,
                16#E3, 16#83, 16#88, 16#E3, 16#83, 16#AB, "i", 16#CC, 16#87, "tel(d)", 16#E3, 16#82, 16#A2,
                16#E3, 16#83, 16#91, 16#E3, 16#83, 16#BC, 16#E3, 16#83, 16#88>>,
              <<"xss", 16#E3, 16#82, 16#AD, 16#E3, 16#83, 16#AD, 16#E3, 16#83, 16#A1, 16#E3, 16#83, 16#BC,
                16#E3, 16#83, 16#88, 16#E3, 16#83, 16#AB, "i", 16#CC, 16#87, "tel(d)", 16#E3, 16#82, 16#A2,
                16#E3, 16#83, 16#91, 16#E3, 16#83, 16#BC, 16#E3, 16#83, 16#88>>,
              <<"X", 16#C3, 16#9F, 16#E3, 16#82, 16#AD, 16#E3, 16#83, 16#AD, 16#E3, 16#83, 16#A1, 16#E3,
                16#83, 16#BC, 16#E3, 16#83, 16#88, 16#E3, 16#83, 16#AB, 16#C4, 16#B0, "TEL(d)", 16#E3, 16#82,
                16#A2, 16#E3, 16#83, 16#91, 16#E3, 16#83, 16#BC, 16#E3, 16#83, 16#88>>}
            ],
    lists:foreach(
        fun({Arg, Name, Node, Resource}) ->
            assert_eq("nameprep", Name, stringprep_rust:nameprep(Arg), Arg),
            assert_eq("nodeprep", Node, stringprep_rust:nodeprep(Arg), Arg),
            assert_eq("resourceprep", Resource, stringprep_rust:resourceprep(Arg), Arg),
            %% tolower should produce same result as nameprep
            assert_eq("tolower", Name, stringprep_rust:tolower(Arg), Arg)
        end,
        Cases).

assert_eq(FnName, Expected, Got, Input) ->
    case Expected =:= Got of
        true -> ok;
        false ->
            error({assertion_failed,
                   [{function, FnName},
                    {input, Input},
                    {expected, Expected},
                    {got, Got}]})
    end.

cache_stats_test() ->
    %% After running vector_test, cache should have entries
    _ = stringprep_rust:nodeprep(<<"test-cache">>),
    _ = stringprep_rust:nodeprep(<<"test-cache">>), %% cache hit
    Size = stringprep_rust:cache_size(),
    true = (Size > 0),
    {Hits, Misses} = stringprep_rust:cache_stats(),
    true = (Hits > 0),
    true = (Misses > 0),
    ok.

iolist_input_test() ->
    %% iolist input should work (converted to binary by Erlang wrapper)
    <<"hello">> = stringprep_rust:nodeprep([<<"hel">>, <<"lo">>]),
    <<"hello">> = stringprep_rust:nameprep([$h, $e, $l, $l, $o]),
    <<"hello">> = stringprep_rust:tolower([<<"HEL">>, <<"LO">>]),
    ok.
