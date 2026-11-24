using System;
using System.Linq;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.Loader;

namespace Antmicro.Renode.Core
{
    public interface IExternal { }

    public sealed class Emulation
    {
        public ExternalsManager ExternalsManager { get; } = new();
        public int GetSeed() => 0;
    }

    public sealed class ExternalsManager
    {
        private readonly System.Collections.Generic.Dictionary<string, IExternal> externals = new();
        public void AddExternal(IExternal external, string name) => externals.Add(name, external);
    }
}

namespace Antmicro.Renode.Utilities
{
    [AttributeUsage(AttributeTargets.Class)]
    public sealed class ConvertibleAttribute : Attribute { }
}

public static class ExtensionHost
{
    public static void Main(string[] args)
    {
        var emulation = new Antmicro.Renode.Core.Emulation();
        var types = args.Length < 2
            ? Array.Empty<Type>()
            : AssemblyLoadContext.Default.LoadFromAssemblyPath(args[1]).GetTypes();
        var methods = types.SelectMany(type => type.GetMethods(BindingFlags.Public | BindingFlags.Static))
            .Where(method => method.IsDefined(typeof(ExtensionAttribute), false)
                && method.GetParameters()[0].ParameterType == emulation.GetType());
        if (args[0] == "scan")
        {
            foreach (var type in types)
                Console.WriteLine($"T\t{type.FullName}");
            foreach (var method in methods)
                Console.WriteLine($"M\t{method.Name}");
            return;
        }
        if (args[0] == "invoke")
        {
            var method = methods.Single(candidate => candidate.Name == args[2]
                && candidate.GetParameters().Length == 1);
            Console.WriteLine(method.Invoke(null, new object[] { emulation }));
            return;
        }
        throw new InvalidOperationException("unknown extension host command");
    }
}
