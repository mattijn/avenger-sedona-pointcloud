"""Walk the GDAL algorithm registry and dump every algorithm's arguments,
including fields that --json-usage omits (positional, aliases, short name,
hidden-for-CLI)."""
import json, sys
from osgeo import gdal
gdal.UseExceptions()

def arg_info(a):
    t = a.GetType()
    d = dict(name=a.GetName(), type=gdal.AlgorithmArgTypeName(t),
             short_name=a.GetShortName() or None, aliases=list(a.GetAliases() or []),
             positional=a.IsPositional(), required=a.IsRequired(),
             hidden=a.IsHidden(), hidden_for_cli=a.IsHiddenForCLI(),
             hidden_for_api=a.IsHiddenForAPI(), is_input=a.IsInput(), is_output=a.IsOutput(),
             category=a.GetCategory(), choices=list(a.GetChoices() or []),
             mutual_exclusion_group=a.GetMutualExclusionGroup() or None,
             available_in_pipeline_step=a.IsAvailableInPipelineStep())
    if gdal.AlgorithmArgTypeIsList(t):
        d.update(min_count=a.GetMinCount(), max_count=a.GetMaxCount(),
                 packed=a.GetPackedValuesAllowed(), repeated=a.GetRepeatedArgAllowed())
    if a.HasDefaultValue():
        tn = d['type']
        getter = {'boolean': a.GetDefaultAsBoolean, 'integer': a.GetDefaultAsInteger,
                  'real': a.GetDefaultAsDouble, 'string': a.GetDefaultAsString,
                  'string_list': a.GetDefaultAsStringList, 'integer_list': a.GetDefaultAsIntegerList,
                  'real_list': a.GetDefaultAsDoubleList}.get(tn)
        if getter:
            v = getter(); d['default'] = list(v) if isinstance(v, tuple) else v
    return d

def walk(alg):
    out = dict(name=alg.GetName(), description=alg.GetDescription(),
               args=[arg_info(alg.GetArg(n)) for n in alg.GetArgNames()], sub=[])
    for s in alg.GetSubAlgorithmNames() or []:
        out['sub'].append(walk(alg.InstantiateSubAlgorithm(s)))
    return out

reg = gdal.GetGlobalAlgorithmRegistry()
tree = [walk(reg.InstantiateAlg(n)) for n in reg.GetAlgNames()]
json.dump(tree, sys.stdout, indent=1, default=str)
