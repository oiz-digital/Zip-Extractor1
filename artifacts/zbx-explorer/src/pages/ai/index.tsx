import React from "react";
import { Link } from "wouter";
import { useGetAiModels, useGetAiInferences, useGetAiStats } from "@workspace/api-client-react";
import { formatNumber, timeAgo, formatAddress } from "@/lib/utils";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Badge } from "@/components/ui/badge";
import { Cpu, Zap, Activity, BrainCircuit } from "lucide-react";

export default function AI() {
  const { data: stats, isLoading: statsLoading } = useGetAiStats();
  const { data: models, isLoading: modelsLoading } = useGetAiModels();
  const { data: inferences, isLoading: inferencesLoading } = useGetAiInferences({ limit: 10 });

  return (
    <div className="space-y-6">
      <div className="flex flex-col md:flex-row justify-between items-start md:items-center gap-4">
        <div>
          <h2 className="text-2xl font-bold tracking-tight flex items-center gap-2">
            <Cpu className="text-primary w-6 h-6" /> On-Chain AI Inference
          </h2>
          <p className="text-muted-foreground">Unique to Zebvix: trustless AI model inference embedded directly in consensus.</p>
        </div>
      </div>

      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Activity className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Total Inferences</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-24" /> : formatNumber(stats?.totalInferences || 0, 0)}
              </h3>
            </div>
          </CardContent>
        </Card>
        
        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><BrainCircuit className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Unique Callers</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-24" /> : formatNumber(stats?.uniqueCallers || 0, 0)}
              </h3>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Cpu className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Top Model</p>
              <h3 className="text-lg font-bold truncate w-24" title={stats?.topModel}>
                {statsLoading ? <Skeleton className="h-7 w-20" /> : stats?.topModel || 'N/A'}
              </h3>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Zap className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Avg Gas Cost</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-24" /> : formatNumber(stats?.avgGasPerInference || 0, 0)}
              </h3>
            </div>
          </CardContent>
        </Card>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        <Card className="flex flex-col h-full">
          <CardHeader>
            <CardTitle>Registered Models</CardTitle>
          </CardHeader>
          <CardContent className="flex-1 overflow-auto">
            {modelsLoading ? (
              <div className="space-y-4">
                {Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-16 w-full" />)}
              </div>
            ) : (
              <div className="space-y-4">
                {models?.map((model) => (
                  <div key={model.id} className="p-4 bg-muted/30 border border-border rounded-lg flex flex-col gap-2">
                    <div className="flex justify-between items-start">
                      <div>
                        <h4 className="font-bold text-primary">{model.name}</h4>
                        <p className="text-xs text-muted-foreground">{model.description}</p>
                      </div>
                      <Badge variant="outline" className="uppercase text-[10px] shrink-0">{model.type}</Badge>
                    </div>
                    <div className="grid grid-cols-3 gap-2 mt-2 pt-2 border-t border-border/50 text-xs">
                      <div>
                        <span className="text-muted-foreground block mb-0.5">Accuracy</span>
                        <span className="font-mono text-green-500">{(model.accuracy * 100).toFixed(1)}%</span>
                      </div>
                      <div>
                        <span className="text-muted-foreground block mb-0.5">Gas Cost</span>
                        <span className="font-mono">{formatNumber(model.gasPerInference, 0)}</span>
                      </div>
                      <div>
                        <span className="text-muted-foreground block mb-0.5">Calls</span>
                        <span className="font-mono">{formatNumber(model.inferenceCount, 0)}</span>
                      </div>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </CardContent>
        </Card>

        <Card className="flex flex-col h-full">
          <CardHeader>
            <CardTitle>Recent Inferences</CardTitle>
          </CardHeader>
          <CardContent className="flex-1 overflow-auto p-0">
            {inferencesLoading ? (
              <div className="p-4 space-y-2">
                {Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}
              </div>
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Tx / Caller</TableHead>
                    <TableHead>Model</TableHead>
                    <TableHead className="text-right">Confidence</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {inferences?.map((inf) => (
                    <TableRow key={inf.txHash}>
                      <TableCell>
                        <div className="flex flex-col">
                          <Link href={`/txs/${inf.txHash}`} className="font-mono text-xs text-primary hover:underline">
                            {formatAddress(inf.txHash, 6)}
                          </Link>
                          <Link href={`/address/${inf.caller}`} className="font-mono text-[10px] text-muted-foreground hover:text-foreground">
                            {formatAddress(inf.caller, 6)}
                          </Link>
                          <span className="text-[10px] text-muted-foreground">{timeAgo(inf.timestamp)}</span>
                        </div>
                      </TableCell>
                      <TableCell>
                        <Badge variant="secondary" className="text-xs">
                          {inf.modelName}
                        </Badge>
                      </TableCell>
                      <TableCell className="text-right font-mono">
                        <span className={inf.confidence > 0.9 ? 'text-green-500' : inf.confidence > 0.7 ? 'text-yellow-500' : 'text-destructive'}>
                          {(inf.confidence * 100).toFixed(2)}%
                        </span>
                      </TableCell>
                    </TableRow>
                  ))}
                  {inferences?.length === 0 && (
                    <TableRow>
                      <TableCell colSpan={3} className="text-center py-8 text-muted-foreground">
                        No recent inferences.
                      </TableCell>
                    </TableRow>
                  )}
                </TableBody>
              </Table>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
